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
