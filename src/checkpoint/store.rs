use std::future::Future;
use std::io;
use std::pin::Pin;

use super::Checkpoint;

/// A boxed future returned by [`CheckpointStore`] methods.
pub type CheckpointFuture<'a, T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send + 'a>>;

/// Async trait for persisting and loading agent checkpoints.
///
/// Implementations can back onto any storage: filesystem, database, cloud, etc.
pub trait CheckpointStore: Send + Sync {
    /// Save a checkpoint. Overwrites any existing checkpoint with the same ID.
    fn save_checkpoint(&self, checkpoint: &Checkpoint) -> CheckpointFuture<'_, ()>;

    /// Load a checkpoint by ID.
    fn load_checkpoint(&self, id: &str) -> CheckpointFuture<'_, Option<Checkpoint>>;

    /// List all checkpoint IDs, most recent first.
    fn list_checkpoints(&self) -> CheckpointFuture<'_, Vec<String>>;

    /// Delete a checkpoint by ID.
    fn delete_checkpoint(&self, id: &str) -> CheckpointFuture<'_, ()>;
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Mutex, MutexGuard};

    use super::*;

    struct InMemoryCheckpointStore {
        data: Mutex<HashMap<String, String>>,
    }

    impl InMemoryCheckpointStore {
        fn new() -> Self {
            Self {
                data: Mutex::new(HashMap::new()),
            }
        }

        fn lock_data(&self) -> io::Result<MutexGuard<'_, HashMap<String, String>>> {
            self.data
                .lock()
                .map_err(|error| io::Error::other(error.to_string()))
        }
    }

    impl CheckpointStore for InMemoryCheckpointStore {
        fn save_checkpoint(&self, checkpoint: &Checkpoint) -> CheckpointFuture<'_, ()> {
            let json = serde_json::to_string(checkpoint).unwrap();
            let id = checkpoint.id.clone();
            Box::pin(async move {
                self.lock_data()?.insert(id, json);
                Ok(())
            })
        }

        fn load_checkpoint(&self, id: &str) -> CheckpointFuture<'_, Option<Checkpoint>> {
            let id = id.to_string();
            Box::pin(async move {
                self.lock_data()?
                    .get(&id)
                    .map(|json| serde_json::from_str(json).map_err(io::Error::other))
                    .transpose()
            })
        }

        fn list_checkpoints(&self) -> CheckpointFuture<'_, Vec<String>> {
            Box::pin(async move { Ok(self.lock_data()?.keys().cloned().collect()) })
        }

        fn delete_checkpoint(&self, id: &str) -> CheckpointFuture<'_, ()> {
            let id = id.to_string();
            Box::pin(async move {
                self.lock_data()?.remove(&id);
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn in_memory_checkpoint_store_roundtrip() {
        let store = InMemoryCheckpointStore::new();
        let checkpoint =
            Checkpoint::new("cp-store-test", "prompt", "provider", "model", &[]).with_turn_count(2);

        store.save_checkpoint(&checkpoint).await.unwrap();

        let ids = store.list_checkpoints().await.unwrap();
        assert_eq!(ids, vec!["cp-store-test".to_string()]);

        let loaded = store
            .load_checkpoint("cp-store-test")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.id, "cp-store-test");
        assert_eq!(loaded.turn_count, 2);

        let missing = store.load_checkpoint("nope").await.unwrap();
        assert!(missing.is_none());

        store.delete_checkpoint("cp-store-test").await.unwrap();
        assert!(store.list_checkpoints().await.unwrap().is_empty());
    }
}
