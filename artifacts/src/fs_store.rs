use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
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

const PROCESS_LOCK_DIR_NAME: &str = "swink-agent-artifact-locks";
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

pub(crate) struct ProcessArtifactLock {
    _file: File,
}

fn global_artifact_locks() -> &'static Mutex<LockMap> {
    static LOCKS: OnceLock<Mutex<LockMap>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn hash_process_lock_part(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
    *hash ^= 0xff;
    *hash = hash.wrapping_mul(FNV_PRIME);
}

fn process_lock_id(root: &Path, session_id: &str, name: &str) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    hash_process_lock_part(&mut hash, root.to_string_lossy().as_bytes());
    hash_process_lock_part(&mut hash, session_id.as_bytes());
    hash_process_lock_part(&mut hash, name.as_bytes());
    hash
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

    fn process_lock_path(&self, session_id: &str, name: &str) -> Result<PathBuf, ArtifactError> {
        validate_session_id(session_id)?;
        validate_artifact_name(name)?;
        let id = process_lock_id(&self.root, session_id, name);
        Ok(std::env::temp_dir()
            .join(PROCESS_LOCK_DIR_NAME)
            .join(format!("{id:016x}.lock")))
    }

    pub(crate) async fn process_artifact_lock(
        &self,
        session_id: &str,
        name: &str,
    ) -> Result<ProcessArtifactLock, ArtifactError> {
        let lock_path = self.process_lock_path(session_id, name)?;
        tokio::task::spawn_blocking(move || {
            let parent = lock_path.parent().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "artifact process lock path has no parent directory",
                )
            })?;
            std::fs::create_dir_all(parent)?;
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&lock_path)?;
            file.lock()?;
            Ok::<_, std::io::Error>(ProcessArtifactLock { _file: file })
        })
        .await
        .map_err(|e| storage_err(std::io::Error::other(e)))?
        .map_err(storage_err)
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
    fn save<'a>(
        &'a self,
        session_id: &'a str,
        name: &'a str,
        data: ArtifactData,
    ) -> swink_agent::ArtifactFuture<'a, ArtifactVersion> {
        Box::pin(async move {
            let dir = self.resolve_artifact_dir(session_id, name)?;

            let lock = self.artifact_lock(session_id, name).await;
            let _guard = lock.lock().await;
            let _process_guard = self.process_artifact_lock(session_id, name).await?;

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
            let version = ArtifactVersion::new(
                name.to_string(),
                next_version,
                now,
                data.content.len(),
                data.content_type,
            );
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
        })
    }

    fn load<'a>(
        &'a self,
        session_id: &'a str,
        name: &'a str,
    ) -> swink_agent::ArtifactFuture<'a, Option<(ArtifactData, ArtifactVersion)>> {
        Box::pin(async move {
            self.resolve_artifact_dir(session_id, name)?;

            let lock = self.artifact_lock(session_id, name).await;
            let _guard = lock.lock().await;
            let _process_guard = self.process_artifact_lock(session_id, name).await?;

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

            let data = ArtifactData::new(content, record.content_type.clone())
                .with_metadata(record.metadata.clone());
            let version = ArtifactVersion::new(
                record.name.clone(),
                record.version,
                record.created_at,
                record.size,
                record.content_type.clone(),
            );

            tracing::debug!(
                session_id,
                name,
                version = record.version,
                "artifact loaded"
            );
            Ok(Some((data, version)))
        })
    }

    fn load_version<'a>(
        &'a self,
        session_id: &'a str,
        name: &'a str,
        version: u32,
    ) -> swink_agent::ArtifactFuture<'a, Option<(ArtifactData, ArtifactVersion)>> {
        Box::pin(async move {
            self.resolve_artifact_dir(session_id, name)?;

            let lock = self.artifact_lock(session_id, name).await;
            let _guard = lock.lock().await;
            let _process_guard = self.process_artifact_lock(session_id, name).await?;

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

            let data = ArtifactData::new(content, record.content_type.clone())
                .with_metadata(record.metadata.clone());
            let artifact_version = ArtifactVersion::new(
                record.name.clone(),
                record.version,
                record.created_at,
                record.size,
                record.content_type.clone(),
            );

            tracing::debug!(session_id, name, version, "artifact version loaded");
            Ok(Some((data, artifact_version)))
        })
    }

    fn list<'a>(
        &'a self,
        session_id: &'a str,
    ) -> swink_agent::ArtifactFuture<'a, Vec<ArtifactMeta>> {
        Box::pin(async move {
            let names = self.discover_artifacts(session_id).await?;

            let mut metas = Vec::with_capacity(names.len());
            for name in &names {
                let lock = self.artifact_lock(session_id, name).await;
                let _guard = lock.lock().await;
                let _process_guard = self.process_artifact_lock(session_id, name).await?;

                let meta = self.read_meta(session_id, name).await?;
                self.reject_metadata_content_mismatch(session_id, name, &meta)
                    .await?;
                if let (Some(first), Some(last)) = (meta.versions.first(), meta.versions.last()) {
                    metas.push(ArtifactMeta::new(
                        name.clone(),
                        last.version,
                        first.created_at,
                        last.created_at,
                        last.content_type.clone(),
                    ));
                }
            }

            tracing::debug!(session_id, count = metas.len(), "artifacts listed");
            Ok(metas)
        })
    }

    fn delete<'a>(
        &'a self,
        session_id: &'a str,
        name: &'a str,
    ) -> swink_agent::ArtifactFuture<'a, ()> {
        Box::pin(async move {
            let dir = self.resolve_artifact_dir(session_id, name)?;
            let lock = self.artifact_lock(session_id, name).await;
            let _guard = lock.lock().await;
            let _process_guard = self.process_artifact_lock(session_id, name).await?;

            self.delete_artifact_files(&dir).await?;
            self.prune_empty_artifact_dirs(session_id, name).await?;

            tracing::debug!(session_id, name, "artifact deleted");
            Ok(())
        })
    }
}

#[cfg(test)]
#[path = "fs_store_tests.rs"]
mod tests;
