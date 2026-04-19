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
    if id.contains('/') || id.contains('\\') || id.contains("..") || id.contains('\0') {
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
/// access and therefore must not contain path separators, `..`, or null bytes.
pub struct FileCheckpointStore {
    checkpoints_dir: PathBuf,
}

impl FileCheckpointStore {
    /// Create a new store rooted at the given directory.
    ///
    /// Creates the directory (and parents) if it does not already exist.
    pub fn new(checkpoints_dir: PathBuf) -> io::Result<Self> {
        std::fs::create_dir_all(&checkpoints_dir)?;
        Ok(Self { checkpoints_dir })
    }

    /// Default checkpoints directory: `<config_dir>/swink-agent/checkpoints`.
    pub fn default_dir() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("swink-agent").join("checkpoints"))
    }
}

impl CheckpointStore for FileCheckpointStore {
    fn save_checkpoint(&self, checkpoint: Checkpoint) -> CheckpointFuture<'_, ()> {
        Box::pin(async move {
            validate_checkpoint_id(&checkpoint.id)?;
            let path = checkpoint_path(&self.checkpoints_dir, &checkpoint.id);
            atomic_write(&path, |writer| {
                serde_json::to_writer_pretty(&mut *writer, &checkpoint).map_err(io::Error::other)
            })
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

    use super::FileCheckpointStore;
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
    async fn file_checkpoint_store_rejects_unsafe_checkpoint_ids() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileCheckpointStore::new(dir.path().to_path_buf()).unwrap();

        let err = store.load_checkpoint("../escape").await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
}
