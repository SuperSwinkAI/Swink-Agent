//! Tests for `atomic_fs`.
#![cfg(test)]

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
