use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};

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

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn missing_content_err(
    session_id: &str,
    name: &str,
    version: u32,
    path: &Path,
) -> ArtifactError {
    storage_err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!(
            "artifact '{name}' in session '{session_id}' metadata references missing content for version {version}: {}",
            path.display()
        ),
    ))
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn orphan_content_err(
    session_id: &str,
    name: &str,
    version: u32,
    path: &Path,
) -> ArtifactError {
    storage_err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!(
            "artifact '{name}' in session '{session_id}' has content for version {version} without metadata membership: {}",
            path.display()
        ),
    ))
}

fn invalid_meta_err(session_id: &str, name: &str, reason: impl Into<String>) -> ArtifactError {
    storage_err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!(
            "artifact '{name}' in session '{session_id}' has invalid metadata: {}",
            reason.into()
        ),
    ))
}

fn parse_version_file_name(file_name: &OsStr) -> Option<u32> {
    let name = file_name.to_str()?;
    let version = name.strip_prefix('v')?.strip_suffix(".bin")?;
    version.parse().ok()
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
#[derive(Hash, Eq, PartialEq)]
struct ArtifactLockKey {
    root: PathBuf,
    session_id: String,
    name: String,
}

type LockMap = HashMap<ArtifactLockKey, Weak<Mutex<()>>>;

fn global_artifact_locks() -> &'static Mutex<LockMap> {
    static LOCKS: OnceLock<Mutex<LockMap>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub struct FileArtifactStore {
    root: PathBuf,
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
        Ok(Self { root: canonical })
    }

    /// Get or create the per-artifact lock for a (session, name) pair.
    pub(crate) async fn artifact_lock(&self, session_id: &str, name: &str) -> Arc<Mutex<()>> {
        let key = ArtifactLockKey {
            root: self.root.clone(),
            session_id: session_id.to_string(),
            name: name.to_string(),
        };
        let mut locks = global_artifact_locks().lock().await;
        if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
            return lock;
        }

        locks.retain(|_, lock| lock.strong_count() > 0);

        let lock = Arc::new(Mutex::new(()));
        locks.insert(key, Arc::downgrade(&lock));
        lock
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
    /// Recursively discovers artifact directories (those containing `meta.json`
    /// or direct `vN.bin` content files), returning artifact names relative to
    /// the session directory.
    async fn discover_artifacts(&self, session_id: &str) -> Result<Vec<String>, ArtifactError> {
        let session_dir = self.resolve_session_dir(session_id)?;
        if !session_dir.exists() {
            return Ok(Vec::new());
        }

        let mut names = Vec::new();
        let mut stack = vec![session_dir.clone()];

        while let Some(dir) = stack.pop() {
            let mut entries = tokio::fs::read_dir(&dir).await.map_err(storage_err)?;
            let mut is_artifact_dir = false;
            while let Some(entry) = entries.next_entry().await.map_err(storage_err)? {
                let path = entry.path();
                let file_type = entry.file_type().await.map_err(storage_err)?;
                if file_type.is_dir() {
                    // Recurse into child directories for nested artifact names like
                    // "tool/output".
                    stack.push(path);
                } else if file_type.is_file()
                    && (entry.file_name() == "meta.json"
                        || parse_version_file_name(&entry.file_name()).is_some())
                {
                    is_artifact_dir = true;
                }
            }

            if is_artifact_dir
                && let Ok(relative) = dir.strip_prefix(&session_dir)
                && let Some(name) = relative.to_str()
                && !name.is_empty()
            {
                names.push(name.replace('\\', "/"));
            }
        }

        names.sort();
        names.dedup();

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

    pub(crate) async fn rollback_version_file(path: &Path) {
        match tokio::fs::remove_file(path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "failed to roll back artifact content after metadata write failure"
                );
            }
        }
    }

    async fn reject_orphan_content_files(
        &self,
        session_id: &str,
        name: &str,
        known_versions: &HashSet<u32>,
    ) -> Result<(), ArtifactError> {
        let artifact_dir = self.resolve_artifact_dir(session_id, name)?;
        let mut entries = match tokio::fs::read_dir(&artifact_dir).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(storage_err(error)),
        };

        while let Some(entry) = entries.next_entry().await.map_err(storage_err)? {
            let file_type = entry.file_type().await.map_err(storage_err)?;
            if !file_type.is_file() {
                continue;
            }

            let Some(version) = parse_version_file_name(&entry.file_name()) else {
                continue;
            };
            if !known_versions.contains(&version) {
                return Err(orphan_content_err(session_id, name, version, &entry.path()));
            }
        }

        Ok(())
    }

    pub(crate) async fn reject_content_files_without_metadata(
        &self,
        session_id: &str,
        name: &str,
        meta: &MetaFile,
    ) -> Result<(), ArtifactError> {
        let known_versions = meta
            .versions
            .iter()
            .map(|record| record.version)
            .collect::<HashSet<_>>();
        self.reject_orphan_content_files(session_id, name, &known_versions)
            .await
    }

    async fn reject_metadata_without_content_files(
        &self,
        session_id: &str,
        name: &str,
        meta: &MetaFile,
    ) -> Result<(), ArtifactError> {
        for record in &meta.versions {
            let content_path = self.version_path(session_id, name, record.version);
            match tokio::fs::metadata(&content_path).await {
                Ok(metadata) if metadata.is_file() => {}
                Ok(_) => {
                    return Err(storage_err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "artifact '{name}' in session '{session_id}' metadata references non-file content for version {}: {}",
                            record.version,
                            content_path.display()
                        ),
                    )));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err(missing_content_err(
                        session_id,
                        name,
                        record.version,
                        &content_path,
                    ));
                }
                Err(error) => return Err(storage_err(error)),
            }
        }

        Ok(())
    }

    pub(crate) async fn reject_metadata_content_mismatch(
        &self,
        session_id: &str,
        name: &str,
        meta: &MetaFile,
    ) -> Result<(), ArtifactError> {
        self.reject_content_files_without_metadata(session_id, name, meta)
            .await?;
        self.reject_metadata_without_content_files(session_id, name, meta)
            .await
    }

    pub(crate) async fn next_version(
        &self,
        session_id: &str,
        name: &str,
        meta: &MetaFile,
    ) -> Result<u32, ArtifactError> {
        let mut expected = 1u32;
        let mut known_versions = HashSet::with_capacity(meta.versions.len());
        for record in &meta.versions {
            if record.version != expected {
                return Err(invalid_meta_err(
                    session_id,
                    name,
                    format!(
                        "expected version {expected} in sequence, found version {}",
                        record.version
                    ),
                ));
            }
            known_versions.insert(record.version);
            expected = expected.checked_add(1).ok_or_else(|| {
                invalid_meta_err(session_id, name, "version sequence exceeds u32 range")
            })?;
        }

        self.reject_orphan_content_files(session_id, name, &known_versions)
            .await?;
        self.reject_metadata_without_content_files(session_id, name, meta)
            .await?;

        Ok(expected)
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

        let next_version = self.next_version(session_id, name, &meta).await?;
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
        if let Err(error) = self.write_meta(session_id, name, &meta).await {
            Self::rollback_version_file(&content_path).await;
            return Err(error);
        }

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

        let lock = self.artifact_lock(session_id, name).await;
        let _guard = lock.lock().await;

        let meta = self.read_meta(session_id, name).await?;
        self.reject_metadata_content_mismatch(session_id, name, &meta)
            .await?;
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
                return Err(missing_content_err(
                    session_id,
                    name,
                    record.version,
                    &content_path,
                ));
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

        let lock = self.artifact_lock(session_id, name).await;
        let _guard = lock.lock().await;

        let meta = self.read_meta(session_id, name).await?;
        self.reject_metadata_content_mismatch(session_id, name, &meta)
            .await?;
        let Some(record) = meta.versions.iter().find(|r| r.version == version) else {
            let content_path = self.version_path(session_id, name, version);
            if tokio::fs::try_exists(&content_path)
                .await
                .map_err(storage_err)?
            {
                return Err(orphan_content_err(session_id, name, version, &content_path));
            }
            tracing::debug!(session_id, name, version, "version not found");
            return Ok(None);
        };

        let content_path = self.version_path(session_id, name, version);
        let content = match tokio::fs::read(&content_path).await {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(session_id, name, version, "content file missing");
                return Err(missing_content_err(
                    session_id,
                    name,
                    version,
                    &content_path,
                ));
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
            let lock = self.artifact_lock(session_id, name).await;
            let _guard = lock.lock().await;

            let meta = self.read_meta(session_id, name).await?;
            self.reject_metadata_content_mismatch(session_id, name, &meta)
                .await?;
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
    use std::io::ErrorKind;
    use std::sync::Arc;

    use chrono::Utc;
    use tokio::sync::oneshot;
    use tokio::task::yield_now;

    use super::{FileArtifactStore, VersionRecord};
    use swink_agent::{ArtifactData, ArtifactError, ArtifactStore};

    fn text_data(content: &str) -> ArtifactData {
        ArtifactData {
            content: content.as_bytes().to_vec(),
            content_type: "text/plain".to_string(),
            metadata: HashMap::new(),
        }
    }

    fn assert_invalid_data_storage_error(err: ArtifactError, expected_snippet: &str) {
        let ArtifactError::Storage(source) = err else {
            panic!("expected storage error, got {err:?}");
        };
        let io = source
            .downcast_ref::<std::io::Error>()
            .expect("storage error should wrap std::io::Error");
        assert_eq!(io.kind(), ErrorKind::InvalidData);
        assert!(
            io.to_string().contains(expected_snippet),
            "expected error message to contain '{expected_snippet}', got '{io}'"
        );
    }

    async fn assert_delete_waits_for_lock<T>(
        delete_task: &tokio::task::JoinHandle<T>,
        reason: &str,
    ) {
        yield_now().await;
        assert!(!delete_task.is_finished(), "{reason}");
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
        assert_delete_waits_for_lock(
            &delete_task,
            "delete should wait for the per-artifact lock before removing files",
        )
        .await;

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

    #[tokio::test]
    async fn artifact_locks_are_shared_across_store_instances_for_same_root() {
        let tmpdir = tempfile::TempDir::new().expect("tempdir");
        let first_store = Arc::new(FileArtifactStore::new(tmpdir.path()));
        let second_store = Arc::new(FileArtifactStore::new(tmpdir.path()));
        first_store
            .save("s1", "report.md", text_data("v1"))
            .await
            .expect("initial save");

        let lock = first_store.artifact_lock("s1", "report.md").await;
        let guard = lock.lock().await;

        let (started_tx, started_rx) = oneshot::channel();
        let delete_task = tokio::spawn({
            let second_store = Arc::clone(&second_store);
            async move {
                started_tx.send(()).expect("notify delete start");
                second_store.delete("s1", "report.md").await
            }
        });

        started_rx.await.expect("delete task started");
        assert_delete_waits_for_lock(
            &delete_task,
            "a second store instance should wait on the root-wide artifact lock",
        )
        .await;

        drop(guard);

        delete_task
            .await
            .expect("delete task join")
            .expect("delete should succeed after lock release");

        assert!(
            first_store
                .load("s1", "report.md")
                .await
                .expect("load after delete")
                .is_none(),
            "artifact should be deleted once the cross-instance lock is released"
        );
    }

    #[tokio::test]
    async fn load_returns_invalid_data_when_latest_content_file_is_missing() {
        let tmpdir = tempfile::TempDir::new().expect("tempdir");
        let store = FileArtifactStore::new(tmpdir.path());
        store
            .save("s1", "report.md", text_data("v1"))
            .await
            .expect("save should succeed");

        let content_path = store.version_path("s1", "report.md", 1);
        tokio::fs::remove_file(&content_path)
            .await
            .expect("content file should be removable");

        let err = store
            .load("s1", "report.md")
            .await
            .expect_err("missing content should be surfaced as corruption");
        assert_invalid_data_storage_error(err, "metadata references missing content");
    }

    #[tokio::test]
    async fn load_version_returns_invalid_data_when_content_file_is_missing() {
        let tmpdir = tempfile::TempDir::new().expect("tempdir");
        let store = FileArtifactStore::new(tmpdir.path());
        store
            .save("s1", "report.md", text_data("v1"))
            .await
            .expect("save should succeed");

        let content_path = store.version_path("s1", "report.md", 1);
        tokio::fs::remove_file(&content_path)
            .await
            .expect("content file should be removable");

        let err = store
            .load_version("s1", "report.md", 1)
            .await
            .expect_err("missing content should be surfaced as corruption");
        assert_invalid_data_storage_error(err, "metadata references missing content");
    }

    #[tokio::test]
    async fn load_version_waits_for_in_flight_metadata_commit() {
        let tmpdir = tempfile::TempDir::new().expect("tempdir");
        let store = Arc::new(FileArtifactStore::new(tmpdir.path()));
        store
            .save("s1", "report.md", text_data("v1"))
            .await
            .expect("initial save");

        let lock = store.artifact_lock("s1", "report.md").await;
        let guard = lock.lock().await;

        let content_path = store.version_path("s1", "report.md", 2);
        tokio::fs::write(&content_path, b"v2")
            .await
            .expect("stage new content before metadata commit");

        let mut meta = store
            .read_meta("s1", "report.md")
            .await
            .expect("read current metadata");
        let now = Utc::now();
        meta.versions.push(VersionRecord {
            name: "report.md".to_string(),
            version: 2,
            created_at: now,
            size: 2,
            content_type: "text/plain".to_string(),
            metadata: HashMap::new(),
        });

        let (started_tx, started_rx) = oneshot::channel();
        let load_store = Arc::clone(&store);
        let load_task = tokio::spawn(async move {
            started_tx.send(()).expect("notify load start");
            load_store.load_version("s1", "report.md", 2).await
        });

        started_rx.await.expect("load task started");
        yield_now().await;
        assert!(
            !load_task.is_finished(),
            "load_version should wait for the artifact lock while metadata is being committed"
        );

        store
            .write_meta("s1", "report.md", &meta)
            .await
            .expect("commit metadata");
        drop(guard);

        let (data, version) = load_task
            .await
            .expect("load task join")
            .expect("load should succeed after metadata commit")
            .expect("version should exist after metadata commit");
        assert_eq!(data.content, b"v2");
        assert_eq!(version.version, 2);
    }

    #[tokio::test]
    async fn list_waits_for_in_flight_metadata_commit() {
        let tmpdir = tempfile::TempDir::new().expect("tempdir");
        let store = Arc::new(FileArtifactStore::new(tmpdir.path()));
        store
            .save("s1", "report.md", text_data("v1"))
            .await
            .expect("initial save");

        let lock = store.artifact_lock("s1", "report.md").await;
        let guard = lock.lock().await;

        let content_path = store.version_path("s1", "report.md", 2);
        tokio::fs::write(&content_path, b"v2")
            .await
            .expect("stage new content before metadata commit");

        let mut meta = store
            .read_meta("s1", "report.md")
            .await
            .expect("read current metadata");
        let now = Utc::now();
        meta.versions.push(VersionRecord {
            name: "report.md".to_string(),
            version: 2,
            created_at: now,
            size: 2,
            content_type: "text/plain".to_string(),
            metadata: HashMap::new(),
        });

        let (started_tx, started_rx) = oneshot::channel();
        let list_store = Arc::clone(&store);
        let list_task = tokio::spawn(async move {
            started_tx.send(()).expect("notify list start");
            list_store.list("s1").await
        });

        started_rx.await.expect("list task started");
        yield_now().await;
        assert!(
            !list_task.is_finished(),
            "list should wait for the artifact lock while metadata is being committed"
        );

        store
            .write_meta("s1", "report.md", &meta)
            .await
            .expect("commit metadata");
        drop(guard);

        let artifacts = list_task
            .await
            .expect("list task join")
            .expect("list should succeed after metadata commit");
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].name, "report.md");
        assert_eq!(artifacts[0].latest_version, 2);
    }
}
