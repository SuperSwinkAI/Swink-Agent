//! Atomic file-write helpers shared across workspace crates.
//!
//! Provides [`atomic_write`] and [`atomic_write_bytes`] — both write to a
//! unique temporary file, sync file contents, rename over the target, and
//! sync the parent directory where the platform exposes directory fsync.
//! On error the temp file is removed so an interrupted write never leaves a
//! partial or zero-length file at the target path.
//!
//! Concurrent writes to the **same target** within one process are serialized
//! via a per-path mutex.  Writes to different targets remain fully concurrent.

use std::collections::HashMap;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use tempfile::NamedTempFile;

// ── public API ───────────────────────────────────────────────────────

/// Write to `target` atomically.
///
/// `contents_fn` receives a buffered writer backed by a unique temp file.
/// After it returns successfully the temp file contents are synced, the temp
/// file is renamed over `target`, and the parent directory is synced where the
/// platform supports that durability step. If `contents_fn` fails (or panics)
/// the temp file is cleaned up.
///
/// Concurrent calls targeting the same path are serialized internally;
/// distinct paths are fully concurrent.
pub fn atomic_write<F>(target: &Path, contents_fn: F) -> io::Result<()>
where
    F: FnOnce(&mut BufWriter<&std::fs::File>) -> io::Result<()>,
{
    with_target_lock(target, || atomic_write_inner(target, contents_fn))
}

/// Convenience wrapper: atomically write raw bytes to `target`.
pub fn atomic_write_bytes(target: &Path, data: &[u8]) -> io::Result<()> {
    atomic_write(target, |w| w.write_all(data))
}

/// Execute `op` while holding the per-target lock for `target`.
///
/// Use this when you need to perform multiple operations atomically
/// (e.g. read-modify-write) and call [`atomic_write_unlocked`] inside.
pub fn with_target_lock<T>(target: &Path, op: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    let lock = lock_for_target(target);
    let _guard = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    op()
}

// ── internals ────────────────────────────────────────────────────────

/// Atomic write **without** per-target locking.
///
/// Use this when you already hold the lock returned by [`lock_for_target`]
/// and need to avoid double-locking.
pub fn atomic_write_unlocked<F>(target: &Path, contents_fn: F) -> io::Result<()>
where
    F: FnOnce(&mut BufWriter<&std::fs::File>) -> io::Result<()>,
{
    atomic_write_inner(target, contents_fn)
}

fn atomic_write_inner<F>(target: &Path, contents_fn: F) -> io::Result<()>
where
    F: FnOnce(&mut BufWriter<&std::fs::File>) -> io::Result<()>,
{
    let parent = target.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "target path has no parent directory",
        )
    })?;
    target.file_name().and_then(|s| s.to_str()).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "target path has no file name")
    })?;

    (|| {
        let tmp_file = NamedTempFile::new_in(parent)?;
        {
            let file = tmp_file.as_file();
            let mut writer = BufWriter::new(file);
            contents_fn(&mut writer)?;
            writer.flush()?;
        }
        tmp_file.as_file().sync_all()?;
        // Preserve an existing target's permissions: NamedTempFile creates
        // 0600-mode files on Unix, so persisting over e.g. a 0755 script would
        // otherwise silently strip its executable/group/world bits. NEW files
        // (no existing target) intentionally keep tempfile's private 0600
        // default — the only new-file creators are internal data stores where
        // private-by-default is fine. Cross-platform: on Windows this clones
        // the readonly flag.
        if let Ok(existing) = std::fs::metadata(target) {
            tmp_file.as_file().set_permissions(existing.permissions())?;
        }
        tmp_file.persist(target).map_err(|err| err.error)?;
        #[cfg(unix)]
        sync_parent_dir(parent)?;
        Ok(())
    })()
}

#[cfg(unix)]
fn sync_parent_dir(parent: &Path) -> io::Result<()> {
    std::fs::File::open(parent)?.sync_all()
}

/// Prune dead lock-map entries once the map grows past this many entries.
///
/// Entries whose lock is currently held (or waited on) are never pruned, so
/// the map stays bounded by roughly this threshold plus the number of
/// concurrently in-flight writes.
const LOCK_MAP_PRUNE_THRESHOLD: usize = 64;

/// The global per-target lock map.
///
/// Values are `Weak` so that a lock is owned only by the in-flight writes
/// using it; once the last `Arc` returned by [`lock_for_target`] is dropped
/// the entry is dead and eligible for pruning.
fn lock_map() -> &'static Mutex<HashMap<PathBuf, Weak<Mutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Per-target serialization guard.
///
/// Two overlapping atomic rewrites of the same path inside one process must
/// not race on the final rename step — on Windows this is especially important
/// because the replace sequence is not a single kernel operation. We key a
/// global mutex map on the target path so writes to different sessions remain
/// fully concurrent.
///
/// The map does not grow without bound over the process lifetime: dead
/// entries (no outstanding `Arc`) are pruned opportunistically whenever the
/// map exceeds `LOCK_MAP_PRUNE_THRESHOLD` entries.
///
/// Serialization safety: the lookup-or-insert below runs entirely under the
/// map mutex. A dead `Weak` (upgrade fails) means no thread holds — or can
/// come to hold — the old `Arc`, because entering the critical section
/// requires holding an `Arc` for its whole duration; replacing a dead entry
/// therefore can never yield two live mutexes for the same path.
pub fn lock_for_target(target: &Path) -> Arc<Mutex<()>> {
    let mut guard = lock_map()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if guard.len() >= LOCK_MAP_PRUNE_THRESHOLD {
        guard.retain(|_, weak| weak.strong_count() > 0);
    }
    if let Some(lock) = guard.get(target).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    guard.insert(target.to_path_buf(), Arc::downgrade(&lock));
    lock
}

/// Current number of entries (live or dead) in the lock map.
#[cfg(test)]
fn lock_map_len() -> usize {
    lock_map()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .len()
}

#[cfg(test)]
#[path = "atomic_fs_tests.rs"]
mod tests;
