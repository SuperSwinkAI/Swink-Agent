use std::collections::HashMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use swink_agent::{
    ArtifactData, ArtifactError, ArtifactMeta, ArtifactStore, ArtifactVersion,
    validate_artifact_name, validate_session_id,
};

// ─── Internal meta.json schema ─────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
pub struct MetaFile {
    pub(crate) versions: Vec<VersionRecord>,
}

#[derive(Serialize, Deserialize)]
pub struct VersionRecord {
    pub(crate) name: String,
    pub(crate) version: u32,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) size: usize,
    pub(crate) content_type: String,
    pub(crate) metadata: HashMap<String, String>,
}

// ─── Helpers ────────────────────────────────────────────────────────────────

pub fn storage_err(e: impl Error + Send + Sync + 'static) -> ArtifactError {
    ArtifactError::Storage(Box::new(e))
}

/// Canonicalize `root`, creating it first if it does not already exist.
///
/// This returns an absolute, symlink-free path so later operations can prove
/// their resolved target stays under it.
fn canonicalize_root(root: &Path) -> std::io::Result<PathBuf> {
    if !root.exists() {
        std::fs::create_dir_all(root)?;
    }
    root.canonicalize()
}

/// Ensure `candidate` stays contained within `root` after canonicalization.
///
/// `candidate` need not exist yet. We canonicalize the longest existing
/// ancestor and re-join the remaining components, so newly created
/// subdirectories are still checked for containment once materialized.
fn ensure_within_root(root: &Path, candidate: &Path) -> Result<PathBuf, ArtifactError> {
    // Walk up until we find an existing ancestor we can canonicalize.
    let mut existing = candidate;
    let mut suffix: Vec<&std::ffi::OsStr> = Vec::new();
    let canonical_anchor = loop {
        if let Ok(canonical) = existing.canonicalize() {
            break canonical;
        }
        let Some(name) = existing.file_name() else {
            return Err(ArtifactError::PathOutsideRoot);
        };
        suffix.push(name);
        let Some(parent) = existing.parent() else {
            return Err(ArtifactError::PathOutsideRoot);
        };
        existing = parent;
    };

    let mut resolved = canonical_anchor;
    for component in suffix.iter().rev() {
        resolved.push(component);
    }

    if !resolved.starts_with(root) {
        return Err(ArtifactError::PathOutsideRoot);
    }
    Ok(resolved)
}

// ─── FileArtifactStore ──────────────────────────────────────────────────────

/// Filesystem-backed artifact store with per-artifact locking and atomic writes.
///
/// Artifacts are stored under `{root}/{session_id}/{artifact_name}/` with a
/// `meta.json` sidecar and versioned content files (`v1.bin`, `v2.bin`, ...).
///
/// The `root` is canonicalized on construction (creating the directory if it
/// does not yet exist). Every operation validates `session_id` and verifies
/// that the resolved target path remains under the canonical root, so a
/// crafted `session_id` cannot escape the artifact root.
type LockMap = HashMap<(String, String), Arc<Mutex<()>>>;

pub struct FileArtifactStore {
    root: PathBuf,
    locks: Arc<Mutex<LockMap>>,
}

impl FileArtifactStore {
    /// Create a new file artifact store rooted at the given directory.
    ///
    /// The directory does not need to exist yet — it will be created.
    ///
    /// # Panics
    ///
    /// Panics if the directory cannot be created or canonicalized. Most
    /// callers want [`Self::try_new`] instead, which surfaces I/O errors
    /// as a `Result`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::try_new(root).expect("failed to canonicalize artifact root")
    }

    /// Fallible constructor: returns an error if the root cannot be created
    /// or canonicalized.
    pub fn try_new(root: impl Into<PathBuf>) -> Result<Self, ArtifactError> {
        let root = root.into();
        let canonical = canonicalize_root(&root).map_err(storage_err)?;
        Ok(Self {
            root: canonical,
            locks: Arc::new(Mutex::new(HashMap::new())),
        })
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
    fn meta_path(&self, session_id: &str, name: &str) -> PathBuf {
        self.artifact_dir(session_id, name).join("meta.json")
    }

    /// Path to versioned content file.
    pub(crate) fn version_path(&self, session_id: &str, name: &str, version: u32) -> PathBuf {
        self.artifact_dir(session_id, name)
            .join(format!("v{version}.bin"))
    }

    /// Validate `session_id` and confirm that the resolved `session_dir`
    /// stays beneath the canonical root. Returns the resolved session dir.
    pub(crate) fn resolve_session_dir(&self, session_id: &str) -> Result<PathBuf, ArtifactError> {
        validate_session_id(session_id)?;
        let candidate = self.root.join(session_id);
        ensure_within_root(&self.root, &candidate)
    }

    /// Validate `session_id`/`name` and resolve the artifact directory,
    /// enforcing root containment.
    pub(crate) fn resolve_artifact_dir(
        &self,
        session_id: &str,
        name: &str,
    ) -> Result<PathBuf, ArtifactError> {
        validate_session_id(session_id)?;
        validate_artifact_name(name)?;
        let candidate = self.artifact_dir(session_id, name);
        ensure_within_root(&self.root, &candidate)
    }

    /// Read meta.json, returning an empty `MetaFile` if it doesn't exist.
    pub(crate) async fn read_meta(
        &self,
        session_id: &str,
        name: &str,
    ) -> Result<MetaFile, ArtifactError> {
        self.resolve_artifact_dir(session_id, name)?;
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
        self.resolve_artifact_dir(session_id, name)?;
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
        let session_dir = self.resolve_session_dir(session_id)?;
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
                            names.push(name.replace('\\', "/"));
                        }
                    }
                    // Also recurse into it (for nested artifact names like "tool/output")
                    stack.push(path);
                }
            }
        }

        Ok(names)
    }

    /// Remove only the direct files for one logical artifact directory.
    ///
    /// Child artifact names such as `foo/bar` live in nested directories under
    /// `foo/`, so deleting `foo` must not recurse and wipe those children.
    async fn delete_artifact_files(&self, dir: &Path) -> Result<(), ArtifactError> {
        let mut entries = match tokio::fs::read_dir(dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(storage_err(e)),
        };

        while let Some(entry) = entries.next_entry().await.map_err(storage_err)? {
            let file_type = entry.file_type().await.map_err(storage_err)?;
            if file_type.is_dir() {
                continue;
            }

            match tokio::fs::remove_file(entry.path()).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(storage_err(e)),
            }
        }

        Ok(())
    }

    /// Remove now-empty artifact directories up to, but not including, the
    /// session root. Stops once a parent still contains sibling artifacts.
    async fn prune_empty_artifact_dirs(
        &self,
        session_id: &str,
        name: &str,
    ) -> Result<(), ArtifactError> {
        let session_dir = self.resolve_session_dir(session_id)?;
        let mut current = self.resolve_artifact_dir(session_id, name)?;

        while current != session_dir {
            match tokio::fs::remove_dir(&current).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty => break,
                Err(e) => return Err(storage_err(e)),
            }

            let Some(parent) = current.parent() else {
                break;
            };
            current = parent.to_path_buf();
        }

        Ok(())
    }
}

impl ArtifactStore for FileArtifactStore {
    async fn save(
        &self,
        session_id: &str,
        name: &str,
        data: ArtifactData,
    ) -> Result<ArtifactVersion, ArtifactError> {
        let dir = self.resolve_artifact_dir(session_id, name)?;

        let lock = self.artifact_lock(session_id, name).await;
        let _guard = lock.lock().await;

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
        self.resolve_artifact_dir(session_id, name)?;
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
        self.resolve_artifact_dir(session_id, name)?;
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
        let dir = self.resolve_artifact_dir(session_id, name)?;
        let lock = self.artifact_lock(session_id, name).await;
        let _guard = lock.lock().await;

        self.delete_artifact_files(&dir).await?;
        self.prune_empty_artifact_dirs(session_id, name).await?;

        tracing::debug!(session_id, name, "artifact deleted");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::sync::oneshot;
    use tokio::time::sleep;

    use super::FileArtifactStore;
    use swink_agent::{ArtifactData, ArtifactStore};

    fn text_data(content: &str) -> ArtifactData {
        ArtifactData {
            content: content.as_bytes().to_vec(),
            content_type: "text/plain".to_string(),
            metadata: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn delete_waits_for_in_flight_artifact_lock() {
        let tmpdir = tempfile::TempDir::new().expect("tempdir");
        let store = Arc::new(FileArtifactStore::new(tmpdir.path()));
        store
            .save("s1", "report.md", text_data("v1"))
            .await
            .expect("initial save");

        let lock = store.artifact_lock("s1", "report.md").await;
        let guard = lock.lock().await;

        let (started_tx, started_rx) = oneshot::channel();
        let delete_store = Arc::clone(&store);
        let delete_task = tokio::spawn(async move {
            started_tx.send(()).expect("notify delete start");
            delete_store.delete("s1", "report.md").await
        });

        started_rx.await.expect("delete task started");
        sleep(Duration::from_millis(50)).await;
        let delete_finished = delete_task.is_finished();
        assert!(
            !delete_finished,
            "delete should wait for the per-artifact lock before removing files"
        );

        drop(guard);

        delete_task
            .await
            .expect("delete task join")
            .expect("delete should succeed after lock release");

        assert!(
            store
                .load("s1", "report.md")
                .await
                .expect("load after delete")
                .is_none(),
            "artifact should be deleted once the lock is released"
        );
    }
}
