//! Tests for `path`.
#![cfg(test)]

// Every test below is `#[cfg(unix)]` (they all build symlinks through
// `std::os::unix::fs`), so on Windows this module is empty and the glob
// import would be unused.
#[cfg(unix)]
use super::*;

#[cfg(unix)]
#[tokio::test]
async fn resolve_writable_path_rejects_dangling_symlink_escaping_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    tokio::fs::create_dir(&root).await.unwrap();
    // Dangling symlink inside the root whose (absent) target lives
    // outside the root; writing through it must be rejected.
    let outside_target = temp.path().join("outside").join("authorized_keys");
    std::os::unix::fs::symlink(&outside_target, root.join("notes.txt")).unwrap();

    let error = resolve_writable_path("notes.txt", Some(&root))
        .await
        .expect_err("dangling symlink escaping the execution root must be rejected");

    assert!(
        error.contains("escapes execution root"),
        "unexpected error: {error}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn resolve_writable_path_rejects_relative_dangling_symlink_escaping_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    tokio::fs::create_dir(&root).await.unwrap();
    std::os::unix::fs::symlink("../outside.txt", root.join("notes.txt")).unwrap();

    let error = resolve_writable_path("notes.txt", Some(&root))
        .await
        .expect_err("relative dangling symlink escaping the execution root must be rejected");

    assert!(
        error.contains("escapes execution root"),
        "unexpected error: {error}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn resolve_writable_path_accepts_dangling_symlink_inside_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    tokio::fs::create_dir(&root).await.unwrap();
    std::os::unix::fs::symlink("inside.txt", root.join("notes.txt")).unwrap();

    let resolved = resolve_writable_path("notes.txt", Some(&root))
        .await
        .expect("dangling symlink whose target stays inside the root is writable");

    let canonical_root = tokio::fs::canonicalize(&root).await.unwrap();
    assert!(
        resolved.starts_with(&canonical_root),
        "resolved path {} left the execution root",
        resolved.display()
    );
    assert_eq!(resolved.file_name().unwrap(), "inside.txt");
}
