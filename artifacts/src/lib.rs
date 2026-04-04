#![forbid(unsafe_code)]

//! Versioned artifact storage for swink-agent sessions.
//!
//! Provides [`InMemoryArtifactStore`] for testing and `FileArtifactStore` for
//! persistent storage. Both implement the [`swink_agent::ArtifactStore`] trait
//! defined in the core crate behind the `artifact-store` feature gate.

mod fs_store;
mod memory_store;
mod streaming;
mod validate;

pub use fs_store::FileArtifactStore;
pub use memory_store::InMemoryArtifactStore;
pub use validate::validate_artifact_name;
