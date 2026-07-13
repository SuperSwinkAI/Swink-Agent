# Public API Contract: Artifact Service

**Feature**: 036-artifact-service | **Date**: 2026-04-02

## Core Trait (swink-agent crate, `artifact-store` feature)

### ArtifactStore

The real trait (`src/artifact.rs`) is hand-written with boxed futures rather than
`#[async_trait]` — there is no `async_trait` dependency anywhere in this crate.
Each method takes an explicit `'a` lifetime tied to `&'a self` and its `&'a str`
arguments, and returns the `ArtifactFuture<'a, T>` type alias
(`Pin<Box<dyn Future<Output = Result<T, ArtifactError>> + Send + 'a>>`). This is
what makes the trait object-safe (see T076): `Arc<dyn ArtifactStore>` is used
throughout, e.g. by the built-in tools.

```rust
/// Boxed future returned by artifact store operations.
pub type ArtifactFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ArtifactError>> + Send + 'a>>;

/// Pluggable storage backend for session-attached versioned artifacts.
///
/// All methods are scoped by session ID. Implementations must be safe for
/// concurrent use from multiple tools within the same agent.
pub trait ArtifactStore: Send + Sync {
    /// Save content as a new version of the named artifact.
    ///
    /// Returns the version record on success. Version numbers are
    /// monotonically increasing per artifact per session, starting at 1.
    fn save<'a>(
        &'a self,
        session_id: &'a str,
        name: &'a str,
        data: ArtifactData,
    ) -> ArtifactFuture<'a, ArtifactVersion>;

    /// Load the latest version of the named artifact.
    ///
    /// Returns `None` if the artifact does not exist.
    fn load<'a>(
        &'a self,
        session_id: &'a str,
        name: &'a str,
    ) -> ArtifactFuture<'a, Option<(ArtifactData, ArtifactVersion)>>;

    /// Load a specific version of the named artifact.
    ///
    /// Returns `None` if the artifact or version does not exist.
    fn load_version<'a>(
        &'a self,
        session_id: &'a str,
        name: &'a str,
        version: u32,
    ) -> ArtifactFuture<'a, Option<(ArtifactData, ArtifactVersion)>>;

    /// List metadata for all artifacts in a session.
    ///
    /// Returns an empty vec if the session has no artifacts.
    fn list<'a>(&'a self, session_id: &'a str) -> ArtifactFuture<'a, Vec<ArtifactMeta>>;

    /// Delete all versions of the named artifact.
    ///
    /// Succeeds silently if the artifact does not exist (idempotent).
    fn delete<'a>(&'a self, session_id: &'a str, name: &'a str) -> ArtifactFuture<'a, ()>;
}
```

### StreamingArtifactStore (extension trait)

```rust
/// A boxed byte stream used by `StreamingArtifactStore`.
pub type ArtifactByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, ArtifactError>> + Send>>;

/// Extension trait for artifact stores that support streaming I/O.
///
/// Implementing this trait is optional. The base `ArtifactStore` trait
/// uses `Vec<u8>` for all content operations.
pub trait StreamingArtifactStore: ArtifactStore {
    /// Save content from a byte stream as a new version.
    fn save_stream<'a>(
        &'a self,
        session_id: &'a str,
        name: &'a str,
        content_type: String,
        metadata: HashMap<String, String>,
        stream: ArtifactByteStream,
    ) -> ArtifactFuture<'a, ArtifactVersion>;

    /// Load an artifact version as a byte stream.
    ///
    /// If `version` is `None`, loads the latest version.
    /// Returns `None` if the artifact or version does not exist.
    fn load_stream<'a>(
        &'a self,
        session_id: &'a str,
        name: &'a str,
        version: Option<u32>,
    ) -> ArtifactFuture<'a, Option<ArtifactByteStream>>;
}
```

### Types

```rust
/// Content payload for an artifact save operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactData {
    pub content: Vec<u8>,
    pub content_type: String,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Record describing a specific saved version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactVersion {
    pub name: String,
    pub version: u32,
    pub created_at: DateTime<Utc>,
    pub size: usize,
    pub content_type: String,
}

/// Summary metadata for an artifact (used in list results).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactMeta {
    pub name: String,
    pub latest_version: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub content_type: String,
}

/// Errors from artifact operations.
#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error("invalid artifact name '{name}': {reason}")]
    InvalidName { name: String, reason: String },

    #[error("artifact storage error: {0}")]
    Storage(#[from] Box<dyn std::error::Error + Send + Sync>),

    #[error("artifact store not configured")]
    NotConfigured,
}
```

### AgentEvent Variant

```rust
// Added to the existing AgentEvent enum (behind #[cfg(feature = "artifact-store")])
AgentEvent::ArtifactSaved {
    session_id: String,
    name: String,
    version: u32,
},
```

### Name Validation

```rust
/// Validate an artifact name. Returns `Ok(())` if valid.
///
/// Allowed characters: alphanumeric, hyphens, underscores, dots, forward slashes.
/// Must not be empty, start/end with `/`, or contain `//`.
pub fn validate_artifact_name(name: &str) -> Result<(), ArtifactError>;
```

## Artifacts Crate (swink-agent-artifacts)

### FileArtifactStore

```rust
/// Filesystem-backed artifact store.
///
/// Organizes artifacts as versioned files under a configurable root directory.
/// Thread-safe for concurrent access within a single process.
impl FileArtifactStore {
    /// Create a new store rooted at the given directory path.
    pub fn new(root: impl Into<PathBuf>) -> Self;
}

// Implements: ArtifactStore + StreamingArtifactStore
```

### InMemoryArtifactStore

```rust
/// In-memory artifact store for testing and lightweight use.
///
/// All data lives in heap memory. Not persisted across process restarts.
impl InMemoryArtifactStore {
    /// Create a new empty in-memory store.
    pub fn new() -> Self;
}

// Implements: ArtifactStore (not StreamingArtifactStore)
```

## Built-in Tools (swink-agent crate, `artifact-tools` feature)

### SaveArtifactTool

```rust
impl SaveArtifactTool {
    pub fn new(store: Arc<dyn ArtifactStore>) -> Self;
}
// Tool name: "save_artifact"
// Parameters: { name: String, content: String, content_type?: String }
// Returns: "Saved {name} version {version}" or error
```

### LoadArtifactTool

```rust
impl LoadArtifactTool {
    pub fn new(store: Arc<dyn ArtifactStore>) -> Self;
}
// Tool name: "load_artifact"
// Parameters: { name: String, version?: u32 }
// Returns: text content or "[binary: {size} bytes, type: {type}]"
```

### ListArtifactsTool

```rust
impl ListArtifactsTool {
    pub fn new(store: Arc<dyn ArtifactStore>) -> Self;
}
// Tool name: "list_artifacts"
// Parameters: {} (none required)
// Returns: formatted list or "No artifacts in this session."
```

### Convenience Constructor

```rust
/// Create all built-in artifact tools.
pub fn artifact_tools(store: Arc<dyn ArtifactStore>) -> Vec<Arc<dyn AgentTool>>;
```

## Re-exports (swink-agent lib.rs)

```rust
// Behind #[cfg(feature = "artifact-store")]
pub use artifact::{
    ArtifactData, ArtifactError, ArtifactMeta, ArtifactStore, ArtifactVersion,
    StreamingArtifactStore, validate_artifact_name,
};

// Behind #[cfg(feature = "artifact-tools")]
pub use tools::{
    SaveArtifactTool, LoadArtifactTool, ListArtifactsTool, artifact_tools,
};
```
