use std::path::{Component, Path, PathBuf};

pub(crate) async fn resolve_existing_path(
    path: &str,
    execution_root: Option<&Path>,
) -> Result<PathBuf, String> {
    let Some(root) = execution_root else {
        return Ok(Path::new(path).to_path_buf());
    };

    let root = canonical_root(root).await?;
    let candidate = candidate_path(path, &root)?;
    let canonical = tokio::fs::canonicalize(&candidate)
        .await
        .map_err(|error| format!("failed to resolve path {}: {error}", candidate.display()))?;

    if canonical.starts_with(&root) {
        Ok(canonical)
    } else {
        Err(format!(
            "path {} escapes execution root {}",
            candidate.display(),
            root.display()
        ))
    }
}

pub(crate) async fn resolve_writable_path(
    path: &str,
    execution_root: Option<&Path>,
) -> Result<PathBuf, String> {
    let Some(root) = execution_root else {
        return Ok(Path::new(path).to_path_buf());
    };

    let root = canonical_root(root).await?;
    let candidate = candidate_path(path, &root)?;

    let target_exists = tokio::fs::try_exists(&candidate).await.map_err(|error| {
        format!(
            "failed to check whether path exists {}: {error}",
            candidate.display()
        )
    })?;

    if target_exists {
        let canonical = tokio::fs::canonicalize(&candidate)
            .await
            .map_err(|error| format!("failed to resolve path {}: {error}", candidate.display()))?;
        return if canonical.starts_with(&root) {
            Ok(canonical)
        } else {
            Err(format!(
                "path {} escapes execution root {}",
                candidate.display(),
                root.display()
            ))
        };
    }

    // `try_exists` follows symlinks, so a dangling symlink reports `false`
    // even though an entry exists at the path. Creating the file there would
    // follow the link and write at its target, potentially outside the
    // execution root. Detect that case with a non-following lstat and
    // validate the link target explicitly.
    match tokio::fs::symlink_metadata(&candidate).await {
        Ok(_) => return resolve_dangling_symlink(&candidate, &root).await,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "failed to check whether path exists {}: {error}",
                candidate.display()
            ));
        }
    }

    let ancestor = nearest_existing_ancestor(&candidate).await?;
    let canonical_ancestor = tokio::fs::canonicalize(&ancestor)
        .await
        .map_err(|error| format!("failed to resolve path {}: {error}", ancestor.display()))?;

    if canonical_ancestor.starts_with(&root) {
        Ok(candidate)
    } else {
        Err(format!(
            "path {} escapes execution root {}",
            candidate.display(),
            root.display()
        ))
    }
}

/// Resolve a path for a **read-only preview** (e.g. building approval context),
/// synchronously and without touching the filesystem for writes.
///
/// This is deliberately more conservative than [`resolve_writable_path`]: it
/// fails closed to `None` on any ambiguity rather than returning an error,
/// because callers use it only to enrich a preview. A `None` result means "no
/// preview available", never "write is permitted".
///
/// The blocking `std::fs` calls are acceptable here because this runs on the
/// approval path, which is already gated on human input.
pub(crate) fn resolve_readable_path_blocking(
    path: &str,
    execution_root: Option<&Path>,
) -> Option<PathBuf> {
    let Some(root) = execution_root else {
        return Some(Path::new(path).to_path_buf());
    };

    let root = std::fs::canonicalize(root).ok()?;
    let candidate = candidate_path(path, &root).ok()?;

    // The file may not exist yet (a brand-new file), in which case
    // `canonicalize` fails. Fall back to the lexically normalized candidate,
    // which `candidate_path` has already confined to the root.
    match std::fs::canonicalize(&candidate) {
        Ok(canonical) => canonical.starts_with(&root).then_some(canonical),
        Err(_) => candidate.starts_with(&root).then_some(candidate),
    }
}

/// Validate a dangling symlink at `candidate` (an lstat entry exists but the
/// followed target does not). Writing through it would create the file at the
/// link target, so the target must be provably inside `root`; otherwise the
/// path is rejected with the same escape error used for out-of-root paths.
async fn resolve_dangling_symlink(candidate: &Path, root: &Path) -> Result<PathBuf, String> {
    let escape_error = || {
        format!(
            "path {} escapes execution root {}",
            candidate.display(),
            root.display()
        )
    };

    let link_target = tokio::fs::read_link(candidate)
        .await
        .map_err(|error| format!("failed to resolve path {}: {error}", candidate.display()))?;

    let resolved = if link_target.is_absolute() {
        normalize(&link_target)
    } else {
        let parent = candidate.parent().unwrap_or(candidate);
        normalize(&parent.join(link_target))
    };

    // A chain of dangling symlinks cannot be cheaply proven to stay inside
    // the root; reject it outright.
    if tokio::fs::symlink_metadata(&resolved).await.is_ok() {
        return Err(escape_error());
    }

    if !resolved.starts_with(root) {
        return Err(escape_error());
    }

    let ancestor = nearest_existing_ancestor(&resolved).await?;
    let canonical_ancestor = tokio::fs::canonicalize(&ancestor)
        .await
        .map_err(|error| format!("failed to resolve path {}: {error}", ancestor.display()))?;

    if canonical_ancestor.starts_with(root) {
        Ok(resolved)
    } else {
        Err(escape_error())
    }
}

async fn canonical_root(root: &Path) -> Result<PathBuf, String> {
    tokio::fs::canonicalize(root).await.map_err(|error| {
        format!(
            "failed to resolve execution root {}: {error}",
            root.display()
        )
    })
}

fn candidate_path(path: &str, root: &Path) -> Result<PathBuf, String> {
    let raw = Path::new(path);
    if raw.is_absolute() {
        return Ok(normalize(raw));
    }

    let candidate = normalize(&root.join(raw));
    if candidate.starts_with(root) {
        Ok(candidate)
    } else {
        Err(format!(
            "path {} escapes execution root {}",
            raw.display(),
            root.display()
        ))
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

async fn nearest_existing_ancestor(path: &Path) -> Result<PathBuf, String> {
    let mut current = path.parent().unwrap_or(path).to_path_buf();
    loop {
        match tokio::fs::try_exists(&current).await {
            Ok(true) => return Ok(current),
            Ok(false) => {
                if !current.pop() {
                    return Err(format!(
                        "failed to find existing ancestor for {}",
                        path.display()
                    ));
                }
            }
            Err(error) => {
                return Err(format!(
                    "failed to check whether path exists {}: {error}",
                    current.display()
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
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
}
