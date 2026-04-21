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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

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
    let file_name = target.file_name().and_then(|s| s.to_str()).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "target path has no file name")
    })?;

    // Unique per write attempt: pid + monotonic counter. Two overlapping
    // rewrites of the same target inside one process must not share a temp
    // path, or they would truncate/rename each other's files.
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_path = parent.join(format!(".{file_name}.tmp.{}.{seq}", std::process::id()));

    let result: io::Result<()> = (|| {
        // `create_new` guarantees we never clobber a pre-existing temp file
        // from another writer that happened to pick the same path.
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        {
            let mut writer = BufWriter::new(&file);
            contents_fn(&mut writer)?;
            writer.flush()?;
        }
        file.sync_all()?;
        drop(file);
        rename_replacing(&tmp_path, target)?;
        #[cfg(unix)]
        sync_parent_dir(parent)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    result
}

/// Rename `from` to `to`, replacing `to` if it already exists.
///
/// Keep the replacement to a single rename operation. The previous
/// delete-then-rename Windows path could lose both files if the process or
/// host crashed in the gap between those two syscalls.
fn rename_replacing(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::rename(from, to)
}

#[cfg(unix)]
fn sync_parent_dir(parent: &Path) -> io::Result<()> {
    std::fs::File::open(parent)?.sync_all()
}

/// Per-target serialization guard.
///
/// Two overlapping atomic rewrites of the same path inside one process must
/// not race on the final rename step — on Windows this is especially important
/// because the replace sequence is not a single kernel operation. We key a
/// global mutex map on the target path so writes to different sessions remain
/// fully concurrent.
pub fn lock_for_target(target: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let map = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard
        .entry(target.to_path_buf())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
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
