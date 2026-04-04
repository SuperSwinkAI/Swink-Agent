//! Core artifact types and trait, gated behind the `artifact-store` feature.

use std::collections::HashMap;
use std::pin::Pin;

use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::Stream;

// ─── Error ──────────────────────────────────────────────────────────────────

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

// ─── Types ──────────────────────────────────────────────────────────────────

/// Content payload for an artifact save operation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArtifactData {
    pub content: Vec<u8>,
    pub content_type: String,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Record describing a specific saved version.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArtifactVersion {
    pub name: String,
    pub version: u32,
    pub created_at: DateTime<Utc>,
    pub size: usize,
    pub content_type: String,
}

/// Summary metadata for an artifact (used in list results).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArtifactMeta {
    pub name: String,
    pub latest_version: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub content_type: String,
}

// ─── Trait ───────────────────────────────────────────────────────────────────

/// Pluggable storage backend for session-attached versioned artifacts.
///
/// All methods are scoped by session ID. Implementations must be safe for
/// concurrent use from multiple tools within the same agent.
pub trait ArtifactStore: Send + Sync {
    /// Save content as a new version of the named artifact.
    ///
    /// Returns the version record on success. Version numbers are
    /// monotonically increasing per artifact per session, starting at 1.
    fn save(
        &self,
        session_id: &str,
        name: &str,
        data: ArtifactData,
    ) -> impl std::future::Future<Output = Result<ArtifactVersion, ArtifactError>> + Send;

    /// Load the latest version of the named artifact.
    ///
    /// Returns `None` if the artifact does not exist.
    fn load(
        &self,
        session_id: &str,
        name: &str,
    ) -> impl std::future::Future<
        Output = Result<Option<(ArtifactData, ArtifactVersion)>, ArtifactError>,
    > + Send;

    /// Load a specific version of the named artifact.
    ///
    /// Returns `None` if the artifact or version does not exist.
    fn load_version(
        &self,
        session_id: &str,
        name: &str,
        version: u32,
    ) -> impl std::future::Future<
        Output = Result<Option<(ArtifactData, ArtifactVersion)>, ArtifactError>,
    > + Send;

    /// List metadata for all artifacts in a session.
    ///
    /// Returns an empty vec if the session has no artifacts.
    fn list(
        &self,
        session_id: &str,
    ) -> impl std::future::Future<Output = Result<Vec<ArtifactMeta>, ArtifactError>> + Send;

    /// Delete all versions of the named artifact.
    ///
    /// Succeeds silently if the artifact does not exist (idempotent).
    fn delete(
        &self,
        session_id: &str,
        name: &str,
    ) -> impl std::future::Future<Output = Result<(), ArtifactError>> + Send;
}

/// A boxed byte stream used by [`StreamingArtifactStore`].
pub type ArtifactByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, ArtifactError>> + Send>>;

/// Extension trait for artifact stores that support streaming I/O.
///
/// This allows saving and loading artifact content as byte streams, which is
/// useful for large artifacts that should not be buffered entirely in memory.
pub trait StreamingArtifactStore: ArtifactStore {
    /// Save content from a byte stream as a new version.
    fn save_stream(
        &self,
        session_id: &str,
        name: &str,
        content_type: String,
        metadata: HashMap<String, String>,
        stream: ArtifactByteStream,
    ) -> impl std::future::Future<Output = Result<ArtifactVersion, ArtifactError>> + Send;

    /// Load an artifact version as a byte stream.
    ///
    /// If `version` is `None`, loads the latest version.
    fn load_stream(
        &self,
        session_id: &str,
        name: &str,
        version: Option<u32>,
    ) -> impl std::future::Future<Output = Result<Option<ArtifactByteStream>, ArtifactError>>
           + Send;
}

/// Validate an artifact name. Returns `Ok(())` if valid.
///
/// Allowed characters: alphanumeric, hyphens, underscores, dots, forward slashes.
/// Must not be empty, start/end with `/`, contain `//`, or contain path traversal (`..`).
pub fn validate_artifact_name(name: &str) -> Result<(), ArtifactError> {
    if name.is_empty() {
        return Err(ArtifactError::InvalidName {
            name: name.to_string(),
            reason: "name must not be empty".to_string(),
        });
    }

    if name.starts_with('/') {
        return Err(ArtifactError::InvalidName {
            name: name.to_string(),
            reason: "name must not start with '/'".to_string(),
        });
    }

    if name.ends_with('/') {
        return Err(ArtifactError::InvalidName {
            name: name.to_string(),
            reason: "name must not end with '/'".to_string(),
        });
    }

    if name.contains("//") {
        return Err(ArtifactError::InvalidName {
            name: name.to_string(),
            reason: "name must not contain consecutive slashes".to_string(),
        });
    }

    if name.contains("../") || name.contains("/..") || name == ".." {
        return Err(ArtifactError::InvalidName {
            name: name.to_string(),
            reason: "name must not contain path traversal".to_string(),
        });
    }

    let valid = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/');
    if !valid {
        return Err(ArtifactError::InvalidName {
            name: name.to_string(),
            reason: "name contains invalid characters (allowed: alphanumeric, -, _, ., /)"
                .to_string(),
        });
    }

    Ok(())
}
