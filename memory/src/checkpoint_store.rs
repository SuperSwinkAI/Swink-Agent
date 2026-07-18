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
mod tests {
    use std::io;

    use super::{FileCheckpointStore, validate_checkpoint_id};
    use swink_agent::{Checkpoint, CheckpointStore};

    #[tokio::test]
    async fn file_checkpoint_store_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileCheckpointStore::new(dir.path().to_path_buf()).unwrap();
        let checkpoint =
            Checkpoint::new("cp-file", "prompt", "provider", "model", &[]).with_turn_count(3);

        store.save_checkpoint(checkpoint).await.unwrap();

        let loaded = store.load_checkpoint("cp-file").await.unwrap().unwrap();
        assert_eq!(loaded.id, "cp-file");
        assert_eq!(loaded.turn_count, 3);
    }

    #[tokio::test]
    async fn file_checkpoint_store_lists_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileCheckpointStore::new(dir.path().to_path_buf()).unwrap();

        let mut older = Checkpoint::new("older", "prompt", "provider", "model", &[]);
        older.created_at = 10;
        let mut newer = Checkpoint::new("newer", "prompt", "provider", "model", &[]);
        newer.created_at = 20;

        store.save_checkpoint(older).await.unwrap();
        store.save_checkpoint(newer).await.unwrap();

        let ids = store.list_checkpoints().await.unwrap();
        assert_eq!(ids, vec!["newer".to_string(), "older".to_string()]);
    }

    #[tokio::test]
    async fn file_checkpoint_store_list_skips_unrelated_and_invalid_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileCheckpointStore::new(dir.path().to_path_buf()).unwrap();

        let mut checkpoint = Checkpoint::new("valid", "prompt", "provider", "model", &[]);
        checkpoint.created_at = 10;
        store.save_checkpoint(checkpoint).await.unwrap();

        std::fs::write(dir.path().join("scratch.tmp"), "not a checkpoint").unwrap();
        std::fs::write(dir.path().join("broken.json"), "{not valid json").unwrap();
        std::fs::write(
            dir.path().join("wrong-shape.json"),
            serde_json::json!({"id": "wrong-shape"}).to_string(),
        )
        .unwrap();

        let ids = store.list_checkpoints().await.unwrap();

        assert_eq!(ids, vec!["valid".to_string()]);
    }

    #[tokio::test]
    async fn retention_prunes_oldest_to_limit() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileCheckpointStore::new(dir.path().to_path_buf())
            .unwrap()
            .with_max_checkpoints(2);

        for (id, created_at) in [("cp-old", 10), ("cp-mid", 20), ("cp-new", 30)] {
            let mut checkpoint = Checkpoint::new(id, "prompt", "provider", "model", &[]);
            checkpoint.created_at = created_at;
            store.save_checkpoint(checkpoint).await.unwrap();
        }

        // Oldest pruned first: only the two newest remain.
        let ids = store.list_checkpoints().await.unwrap();
        assert_eq!(ids, vec!["cp-new".to_string(), "cp-mid".to_string()]);
        assert!(!dir.path().join("cp-old.json").exists());
        assert!(dir.path().join("cp-mid.json").exists());
        assert!(dir.path().join("cp-new.json").exists());
    }

    #[tokio::test]
    async fn retention_defaults_to_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileCheckpointStore::new(dir.path().to_path_buf()).unwrap();

        let total = FileCheckpointStore::DEFAULT_MAX_CHECKPOINTS + 5;
        for i in 0..total {
            let mut checkpoint =
                Checkpoint::new(format!("cp-{i:03}"), "prompt", "provider", "model", &[]);
            checkpoint.created_at = u64::try_from(i).unwrap();
            store.save_checkpoint(checkpoint).await.unwrap();
        }

        let ids = store.list_checkpoints().await.unwrap();
        assert_eq!(
            ids.len(),
            FileCheckpointStore::DEFAULT_MAX_CHECKPOINTS,
            "default retention must cap the checkpoint count"
        );
        assert_eq!(
            ids[0],
            format!("cp-{:03}", total - 1),
            "newest checkpoint must survive pruning"
        );
        assert!(
            !ids.contains(&"cp-000".to_string()),
            "oldest checkpoint must be pruned"
        );
    }

    #[tokio::test]
    async fn retention_unbounded_opt_out_keeps_everything() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileCheckpointStore::new(dir.path().to_path_buf())
            .unwrap()
            .unbounded();

        let total = FileCheckpointStore::DEFAULT_MAX_CHECKPOINTS + 5;
        for i in 0..total {
            let mut checkpoint =
                Checkpoint::new(format!("cp-{i:03}"), "prompt", "provider", "model", &[]);
            checkpoint.created_at = u64::try_from(i).unwrap();
            store.save_checkpoint(checkpoint).await.unwrap();
        }

        assert_eq!(
            store.list_checkpoints().await.unwrap().len(),
            total,
            "unbounded store must retain every checkpoint"
        );
    }

    #[tokio::test]
    async fn retention_ignores_foreign_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileCheckpointStore::new(dir.path().to_path_buf())
            .unwrap()
            .with_max_checkpoints(1);

        // Foreign files that must never be deleted, even under pruning
        // pressure: wrong extension, invalid JSON, and JSON that is not a
        // checkpoint.
        std::fs::write(dir.path().join("scratch.tmp"), "not a checkpoint").unwrap();
        std::fs::write(dir.path().join("broken.json"), "{not valid json").unwrap();
        std::fs::write(
            dir.path().join("wrong-shape.json"),
            serde_json::json!({"id": "wrong-shape"}).to_string(),
        )
        .unwrap();

        let mut older = Checkpoint::new("older", "prompt", "provider", "model", &[]);
        older.created_at = 10;
        store.save_checkpoint(older).await.unwrap();
        let mut newer = Checkpoint::new("newer", "prompt", "provider", "model", &[]);
        newer.created_at = 20;
        store.save_checkpoint(newer).await.unwrap();

        let ids = store.list_checkpoints().await.unwrap();
        assert_eq!(ids, vec!["newer".to_string()]);
        assert!(dir.path().join("scratch.tmp").exists());
        assert!(dir.path().join("broken.json").exists());
        assert!(dir.path().join("wrong-shape.json").exists());
    }

    #[tokio::test]
    async fn retention_same_id_overwrite_does_not_prune_survivors() {
        // Rolling-style usage: repeatedly saving the SAME id under retention
        // must not inflate the count or evict unrelated newer checkpoints.
        let dir = tempfile::tempdir().unwrap();
        let store = FileCheckpointStore::new(dir.path().to_path_buf())
            .unwrap()
            .with_max_checkpoints(2);

        let mut other = Checkpoint::new("other", "prompt", "provider", "model", &[]);
        other.created_at = 100;
        store.save_checkpoint(other).await.unwrap();

        for (turn, created_at) in [(1, 50), (2, 60), (3, 70)] {
            let mut rolling = Checkpoint::new("rolling", "prompt", "provider", "model", &[])
                .with_turn_count(turn);
            rolling.created_at = created_at;
            store.save_checkpoint(rolling).await.unwrap();
        }

        let ids = store.list_checkpoints().await.unwrap();
        assert_eq!(ids, vec!["other".to_string(), "rolling".to_string()]);
        let rolling = store.load_checkpoint("rolling").await.unwrap().unwrap();
        assert_eq!(rolling.turn_count, 3, "content must match the latest save");
    }

    #[tokio::test]
    async fn file_checkpoint_store_rejects_unsafe_checkpoint_ids() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileCheckpointStore::new(dir.path().to_path_buf()).unwrap();

        let err = store.load_checkpoint("../escape").await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn validate_checkpoint_id_rejects_colon() {
        let err = validate_checkpoint_id("C:drive").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn validate_checkpoint_id_rejects_control_chars() {
        let err = validate_checkpoint_id("checkpoint\nid").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
}
