//! File-backed checkpoint persistence for [`swink_agent::CheckpointStore`].

use std::io;
use std::path::{Path, PathBuf};

use swink_agent::atomic_fs::atomic_write;
use swink_agent::{Checkpoint, CheckpointFuture, CheckpointStore};

fn checkpoint_path(checkpoints_dir: &Path, id: &str) -> PathBuf {
    checkpoints_dir.join(format!("{id}.json"))
}

fn validate_checkpoint_id(id: &str) -> io::Result<()> {
    if id.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "checkpoint ID must not be empty",
        ));
    }
    if id.contains('/')
        || id.contains('\\')
        || id.contains("..")
        || id.chars().any(|c| c == ':' || c.is_ascii_control())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("checkpoint ID contains unsafe characters: {id:?}"),
        ));
    }
    Ok(())
}

/// Durable checkpoint store that persists one checkpoint per JSON file.
///
/// Checkpoints are written atomically, so a failed save never leaves a partial
/// or zero-length live file behind. Checkpoint IDs are validated before file
/// access and therefore must not contain path separators, `..`, `:`, or ASCII
/// control characters.
///
/// Retention is bounded by default: after each save, only the
/// [`DEFAULT_MAX_CHECKPOINTS`](Self::DEFAULT_MAX_CHECKPOINTS) most recent
/// checkpoints (by `created_at`) are kept. Use
/// [`with_max_checkpoints`](Self::with_max_checkpoints) to change the bound
/// or [`unbounded`](Self::unbounded) to keep every checkpoint.
pub struct FileCheckpointStore {
    checkpoints_dir: PathBuf,
    max_checkpoints: Option<usize>,
}

impl FileCheckpointStore {
    /// Number of most-recent checkpoints a new store retains by default.
    pub const DEFAULT_MAX_CHECKPOINTS: usize = 20;

    /// Create a new store rooted at the given directory.
    ///
    /// Creates the directory (and parents) if it does not already exist.
    ///
    /// The store keeps at most [`Self::DEFAULT_MAX_CHECKPOINTS`] checkpoints;
    /// see [`Self::with_max_checkpoints`] and [`Self::unbounded`].
    pub fn new(checkpoints_dir: PathBuf) -> io::Result<Self> {
        std::fs::create_dir_all(&checkpoints_dir)?;
        Ok(Self {
            checkpoints_dir,
            max_checkpoints: Some(Self::DEFAULT_MAX_CHECKPOINTS),
        })
    }

    /// Default checkpoints directory: `<config_dir>/swink-agent/checkpoints`.
    pub fn default_dir() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("swink-agent").join("checkpoints"))
    }

    /// Keep at most `n` checkpoints, pruning the oldest (by `created_at`)
    /// after each save.
    ///
    /// The default is [`Self::DEFAULT_MAX_CHECKPOINTS`]. The checkpoint just
    /// saved counts toward the limit, so `n` should be at least 1.
    ///
    /// Pruning only considers files in the store directory that parse as
    /// checkpoints; foreign or malformed files are never deleted. Pruning is
    /// best-effort: failures are logged at warn level and never fail the save
    /// that triggered them.
    #[must_use]
    pub fn with_max_checkpoints(mut self, n: usize) -> Self {
        self.max_checkpoints = Some(n);
        self
    }

    /// Disable retention pruning and keep every checkpoint.
    ///
    /// Disk usage then grows without bound: a per-turn checkpoint policy
    /// leaves one growing file per turn, so an N-turn session stores O(N²)
    /// bytes. Prefer a bound unless every historical checkpoint is needed.
    #[must_use]
    pub fn unbounded(mut self) -> Self {
        self.max_checkpoints = None;
        self
    }

    /// Delete the oldest checkpoints (by `created_at`) beyond `keep`.
    ///
    /// Only files that parse as [`Checkpoint`]s are candidates; unreadable,
    /// malformed, or non-`.json` files are skipped, never deleted.
    fn prune_to(&self, keep: usize) {
        let entries = match std::fs::read_dir(&self.checkpoints_dir) {
            Ok(entries) => entries,
            Err(error) => {
                tracing::warn!(error = %error, "checkpoint retention: cannot read store dir");
                return;
            }
        };

        // (created_at, id, path) for every parseable checkpoint file.
        let mut checkpoints = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let Ok(contents) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Ok(checkpoint) = serde_json::from_str::<Checkpoint>(&contents) {
                checkpoints.push((checkpoint.created_at, checkpoint.id, path));
            }
        }

        if checkpoints.len() <= keep {
            return;
        }

        // Newest first (ties broken by id, matching `list_checkpoints`);
        // everything past `keep` is pruned.
        checkpoints.sort_by(|left, right| (right.0, &right.1).cmp(&(left.0, &left.1)));
        for (_, id, path) in checkpoints.drain(keep..) {
            if let Err(error) = std::fs::remove_file(&path)
                && error.kind() != io::ErrorKind::NotFound
            {
                tracing::warn!(
                    checkpoint_id = %id,
                    path = %path.display(),
                    error = %error,
                    "checkpoint retention: failed to prune checkpoint"
                );
            }
        }
    }
}

impl CheckpointStore for FileCheckpointStore {
    fn save_checkpoint(&self, checkpoint: Checkpoint) -> CheckpointFuture<'_, ()> {
        Box::pin(async move {
            validate_checkpoint_id(&checkpoint.id)?;
            let path = checkpoint_path(&self.checkpoints_dir, &checkpoint.id);
            atomic_write(&path, |writer| {
                serde_json::to_writer_pretty(&mut *writer, &checkpoint).map_err(io::Error::other)
            })?;

            if let Some(keep) = self.max_checkpoints {
                self.prune_to(keep);
            }
            Ok(())
        })
    }

    fn load_checkpoint(&self, id: &str) -> CheckpointFuture<'_, Option<Checkpoint>> {
        let id = id.to_string();
        Box::pin(async move {
            validate_checkpoint_id(&id)?;
            let path = checkpoint_path(&self.checkpoints_dir, &id);
            if !path.exists() {
                return Ok(None);
            }

            let contents = std::fs::read_to_string(path)?;
            serde_json::from_str(&contents)
                .map(Some)
                .map_err(io::Error::other)
        })
    }

    fn list_checkpoints(&self) -> CheckpointFuture<'_, Vec<String>> {
        Box::pin(async move {
            let mut checkpoints = Vec::new();

            for entry in std::fs::read_dir(&self.checkpoints_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) != Some("json") {
                    continue;
                }

                let contents = match std::fs::read_to_string(&path) {
                    Ok(contents) => contents,
                    Err(error) => {
                        tracing::warn!(
                            path = %path.display(),
                            error = %error,
                            "skipping unreadable checkpoint file"
                        );
                        continue;
                    }
                };

                match serde_json::from_str::<Checkpoint>(&contents) {
                    Ok(checkpoint) => checkpoints.push((checkpoint.created_at, checkpoint.id)),
                    Err(error) => {
                        tracing::warn!(
                            path = %path.display(),
                            error = %error,
                            "skipping invalid checkpoint file"
                        );
                    }
                }
            }

            checkpoints.sort_by(|left, right| right.cmp(left));
            Ok(checkpoints.into_iter().map(|(_, id)| id).collect())
        })
    }

    fn delete_checkpoint(&self, id: &str) -> CheckpointFuture<'_, ()> {
        let id = id.to_string();
        Box::pin(async move {
            validate_checkpoint_id(&id)?;
            let path = checkpoint_path(&self.checkpoints_dir, &id);
            match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            }
        })
    }
}

#[cfg(test)]
#[path = "checkpoint_store_tests.rs"]
mod tests;
