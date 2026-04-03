/// In-memory artifact store for testing and lightweight use.
///
/// All data lives in heap memory. Not persisted across process restarts.
pub struct InMemoryArtifactStore;

impl InMemoryArtifactStore {
    /// Create a new empty in-memory store.
    pub const fn new() -> Self {
        Self
    }
}

impl Default for InMemoryArtifactStore {
    fn default() -> Self {
        Self::new()
    }
}
