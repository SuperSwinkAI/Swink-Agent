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
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn atomic_write_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("test.txt");

        atomic_write(&target, |w| writeln!(w, "hello")).unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap().trim(), "hello");
    }

    #[test]
    fn atomic_write_replaces_existing() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("test.txt");
        fs::write(&target, "old").unwrap();

        atomic_write_bytes(&target, b"new").unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
    }

    #[test]
    fn atomic_write_cleans_up_on_error() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("fail.txt");

        let err = atomic_write(&target, |_w| Err(io::Error::other("boom")));

        assert!(err.is_err());
        // No file at target
        assert!(!target.exists());
        // No temp files left behind
        let entries: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert!(entries.is_empty());
    }

    #[test]
    fn concurrent_writes_dont_corrupt() {
        use std::thread;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("concurrent.txt");

        let handles: Vec<_> = (0..10)
            .map(|i| {
                let t = target.clone();
                thread::spawn(move || {
                    atomic_write_bytes(&t, format!("writer-{i}").as_bytes()).unwrap();
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // File should contain one complete writer's output (no corruption).
        let content = fs::read_to_string(&target).unwrap();
        assert!(content.starts_with("writer-"));
    }

    #[test]
    fn lock_map_does_not_grow_unboundedly_across_distinct_paths() {
        // Acquire (and immediately release) locks for many more distinct
        // paths than the prune threshold. Dead entries must be evicted
        // rather than accumulating for the process lifetime.
        for i in 0..10_000 {
            let path = PathBuf::from(format!("/lock-map-growth-test/{i}"));
            drop(lock_for_target(&path));
        }

        // Bound: the threshold, plus entries inserted since the last pruning
        // pass, plus locks concurrently held by other tests in this process.
        assert!(
            lock_map_len() <= 2 * LOCK_MAP_PRUNE_THRESHOLD,
            "lock map should stay bounded, got {} entries",
            lock_map_len()
        );
    }

    #[test]
    fn lock_for_target_returns_same_lock_while_held() {
        // Serialization invariant: while one Arc is live, a second call for
        // the same path must return the SAME mutex (never a fresh one).
        let path = PathBuf::from("/lock-map-identity-test/target");
        let first = lock_for_target(&path);
        let second = lock_for_target(&path);
        assert!(
            Arc::ptr_eq(&first, &second),
            "concurrent acquirers of one path must share a single mutex"
        );
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_preserves_existing_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("script.sh");
        fs::write(&target, "#!/bin/sh\necho old\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();

        atomic_write_bytes(&target, b"#!/bin/sh\necho new\n").unwrap();

        let mode = fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    #[cfg(windows)]
    #[test]
    fn atomic_write_replaces_existing_on_windows_without_predelete() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("replace.txt");
        fs::write(&target, "old").unwrap();

        atomic_write_bytes(&target, b"new").unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
    }
}
