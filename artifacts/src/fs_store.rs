use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use swink_agent::{
    ArtifactData, ArtifactError, ArtifactMeta, ArtifactStore, ArtifactVersion,
    validate_artifact_name,
};

// ─── Internal meta.json schema ─────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
pub(crate) struct MetaFile {
    pub(crate) versions: Vec<VersionRecord>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct VersionRecord {
    pub(crate) name: String,
    pub(crate) version: u32,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) size: usize,
    pub(crate) content_type: String,
    pub(crate) metadata: HashMap<String, String>,
}

// ─── Helpers ────────────────────────────────────────────────────────────────

pub(crate) fn storage_err(e: impl Error + Send + Sync + 'static) -> ArtifactError {
    ArtifactError::Storage(Box::new(e))
}

// ─── FileArtifactStore ──────────────────────────────────────────────────────

/// Filesystem-backed artifact store with per-artifact locking and atomic writes.
///
/// Artifacts are stored under `{root}/{session_id}/{artifact_name}/` with a
/// `meta.json` sidecar and versioned content files (`v1.bin`, `v2.bin`, ...).
type LockMap = HashMap<(String, String), Arc<Mutex<()>>>;

pub struct FileArtifactStore {
    root: PathBuf,
    locks: Arc<Mutex<LockMap>>,
}

impl FileArtifactStore {
    /// Create a new file artifact store rooted at the given directory.
    ///
    /// The directory does not need to exist yet — it will be created on first save.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get or create the per-artifact lock for a (session, name) pair.
    pub(crate) async fn artifact_lock(&self, session_id: &str, name: &str) -> Arc<Mutex<()>> {
        let key = (session_id.to_string(), name.to_string());
        let mut locks = self.locks.lock().await;
        locks
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Path to the artifact directory: `{root}/{session_id}/{artifact_name}/`
    pub(crate) fn artifact_dir(&self, session_id: &str, name: &str) -> PathBuf {
        self.root.join(session_id).join(name)
    }

    /// Path to meta.json for an artifact.
    pub(crate) fn meta_path(&self, session_id: &str, name: &str) -> PathBuf {
        self.artifact_dir(session_id, name).join("meta.json")
    }

    /// Path to versioned content file.
    pub(crate) fn version_path(&self, session_id: &str, name: &str, version: u32) -> PathBuf {
        self.artifact_dir(session_id, name)
            .join(format!("v{version}.bin"))
    }

    /// Read meta.json, returning an empty `MetaFile` if it doesn't exist.
    pub(crate) async fn read_meta(
        &self,
        session_id: &str,
        name: &str,
    ) -> Result<MetaFile, ArtifactError> {
        let path = self.meta_path(session_id, name);
        match tokio::fs::read_to_string(&path).await {
            Ok(contents) => serde_json::from_str(&contents).map_err(storage_err),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(MetaFile {
                versions: Vec::new(),
            }),
            Err(e) => Err(storage_err(e)),
        }
    }

    /// Write meta.json atomically via the shared atomic-write helper.
    pub(crate) async fn write_meta(
        &self,
        session_id: &str,
        name: &str,
        meta: &MetaFile,
    ) -> Result<(), ArtifactError> {
        let meta_path = self.meta_path(session_id, name);
        let json = serde_json::to_string_pretty(meta).map_err(storage_err)?;
        let bytes = json.into_bytes();
        tokio::task::spawn_blocking(move || {
            swink_agent::atomic_fs::atomic_write_bytes(&meta_path, &bytes)
        })
        .await
        .map_err(|e| storage_err(std::io::Error::other(e)))?
        .map_err(storage_err)
    }

    /// Scan a session directory to find all artifact names.
    ///
    /// Recursively discovers artifact directories (those containing `meta.json`),
    /// returning artifact names relative to the session directory.
    async fn discover_artifacts(&self, session_id: &str) -> Result<Vec<String>, ArtifactError> {
        let session_dir = self.root.join(session_id);
        if !session_dir.exists() {
            return Ok(Vec::new());
        }

        let mut names = Vec::new();
        let mut stack = vec![session_dir.clone()];

        while let Some(dir) = stack.pop() {
            let mut entries = tokio::fs::read_dir(&dir).await.map_err(storage_err)?;
            while let Some(entry) = entries.next_entry().await.map_err(storage_err)? {
                let path = entry.path();
                if path.is_dir() {
                    // Check if this directory contains meta.json
                    let meta_candidate = path.join("meta.json");
                    if meta_candidate.exists() {
                        // This is an artifact directory — derive the name
                        if let Ok(relative) = path.strip_prefix(&session_dir)
                            && let Some(name) = relative.to_str()
                        {
                            names.push(name.to_string());
                        }
                    }
                    // Also recurse into it (for nested artifact names like "tool/output")
                    stack.push(path);
                }
            }
        }

        Ok(names)
    }
}

impl ArtifactStore for FileArtifactStore {
    async fn save(
        &self,
        session_id: &str,
        name: &str,
        data: ArtifactData,
    ) -> Result<ArtifactVersion, ArtifactError> {
        validate_artifact_name(name)?;

        let lock = self.artifact_lock(session_id, name).await;
        let _guard = lock.lock().await;

        let dir = self.artifact_dir(session_id, name);
        tokio::fs::create_dir_all(&dir).await.map_err(storage_err)?;

        // Read or create meta
        let mut meta = self.read_meta(session_id, name).await?;

        #[allow(clippy::cast_possible_truncation)]
        let next_version = meta.versions.len() as u32 + 1;
        let now = Utc::now();

        let record = VersionRecord {
            name: name.to_string(),
            version: next_version,
            created_at: now,
            size: data.content.len(),
            content_type: data.content_type.clone(),
            metadata: data.metadata.clone(),
        };

        // Write content atomically via the shared helper.
        let content_path = self.version_path(session_id, name, next_version);
        let content_bytes = data.content.clone();
        tokio::task::spawn_blocking({
            let content_path = content_path.clone();
            move || swink_agent::atomic_fs::atomic_write_bytes(&content_path, &content_bytes)
        })
        .await
        .map_err(|e| storage_err(std::io::Error::other(e)))?
        .map_err(storage_err)?;

        // Update meta.json
        let version = ArtifactVersion {
            name: name.to_string(),
            version: next_version,
            created_at: now,
            size: data.content.len(),
            content_type: data.content_type,
        };
        meta.versions.push(record);
        self.write_meta(session_id, name, &meta).await?;

        tracing::info!(
            session_id,
            name,
            version = next_version,
            size = data.content.len(),
            "artifact saved"
        );

        Ok(version)
    }

    async fn load(
        &self,
        session_id: &str,
        name: &str,
    ) -> Result<Option<(ArtifactData, ArtifactVersion)>, ArtifactError> {
        let meta = self.read_meta(session_id, name).await?;
        let Some(record) = meta.versions.last() else {
            tracing::debug!(session_id, name, "artifact not found");
            return Ok(None);
        };

        let content_path = self.version_path(session_id, name, record.version);
        let content = match tokio::fs::read(&content_path).await {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(
                    session_id,
                    name,
                    version = record.version,
                    "content file missing"
                );
                return Ok(None);
            }
            Err(e) => return Err(storage_err(e)),
        };

        let data = ArtifactData {
            content,
            content_type: record.content_type.clone(),
            metadata: record.metadata.clone(),
        };
        let version = ArtifactVersion {
            name: record.name.clone(),
            version: record.version,
            created_at: record.created_at,
            size: record.size,
            content_type: record.content_type.clone(),
        };

        tracing::debug!(
            session_id,
            name,
            version = record.version,
            "artifact loaded"
        );
        Ok(Some((data, version)))
    }

    async fn load_version(
        &self,
        session_id: &str,
        name: &str,
        version: u32,
    ) -> Result<Option<(ArtifactData, ArtifactVersion)>, ArtifactError> {
        let meta = self.read_meta(session_id, name).await?;
        let Some(record) = meta.versions.iter().find(|r| r.version == version) else {
            tracing::debug!(session_id, name, version, "version not found");
            return Ok(None);
        };

        let content_path = self.version_path(session_id, name, version);
        let content = match tokio::fs::read(&content_path).await {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(session_id, name, version, "content file missing");
                return Ok(None);
            }
            Err(e) => return Err(storage_err(e)),
        };

        let data = ArtifactData {
            content,
            content_type: record.content_type.clone(),
            metadata: record.metadata.clone(),
        };
        let artifact_version = ArtifactVersion {
            name: record.name.clone(),
            version: record.version,
            created_at: record.created_at,
            size: record.size,
            content_type: record.content_type.clone(),
        };

        tracing::debug!(session_id, name, version, "artifact version loaded");
        Ok(Some((data, artifact_version)))
    }

    async fn list(&self, session_id: &str) -> Result<Vec<ArtifactMeta>, ArtifactError> {
        let names = self.discover_artifacts(session_id).await?;

        let mut metas = Vec::with_capacity(names.len());
        for name in &names {
            let meta = self.read_meta(session_id, name).await?;
            if let (Some(first), Some(last)) = (meta.versions.first(), meta.versions.last()) {
                metas.push(ArtifactMeta {
                    name: name.clone(),
                    latest_version: last.version,
                    created_at: first.created_at,
                    updated_at: last.created_at,
                    content_type: last.content_type.clone(),
                });
            }
        }

        tracing::debug!(session_id, count = metas.len(), "artifacts listed");
        Ok(metas)
    }

    async fn delete(&self, session_id: &str, name: &str) -> Result<(), ArtifactError> {
        let dir = self.artifact_dir(session_id, name);
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => {
                tracing::debug!(session_id, name, "artifact deleted");
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(session_id, name, "artifact already absent");
            }
            Err(e) => return Err(storage_err(e)),
        }
        Ok(())
    }
}
