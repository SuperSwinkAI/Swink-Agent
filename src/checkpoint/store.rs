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
    fn save_checkpoint(&self, checkpoint: Checkpoint) -> CheckpointFuture<'_, ()>;

    /// Load a checkpoint by ID.
    fn load_checkpoint(&self, id: &str) -> CheckpointFuture<'_, Option<Checkpoint>>;

    /// List all checkpoint IDs, most recent first.
    fn list_checkpoints(&self) -> CheckpointFuture<'_, Vec<String>>;

    /// Delete a checkpoint by ID.
    fn delete_checkpoint(&self, id: &str) -> CheckpointFuture<'_, ()>;
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
