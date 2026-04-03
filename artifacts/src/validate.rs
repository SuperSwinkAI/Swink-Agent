use swink_agent::ArtifactError;

/// Validate an artifact name.
///
/// Allowed characters: alphanumeric, hyphens, underscores, dots, forward slashes.
/// Must not be empty, start/end with `/`, or contain `//` or `../`.
pub fn validate_artifact_name(name: &str) -> Result<(), ArtifactError> {
    if name.is_empty() {
        return Err(ArtifactError::InvalidName {
            name: name.to_string(),
            reason: "name must not be empty".to_string(),
        });
    }

    if name.starts_with('/') {
        return Err(ArtifactError::InvalidName {
            name: name.to_string(),
            reason: "name must not start with '/'".to_string(),
        });
    }

    if name.ends_with('/') {
        return Err(ArtifactError::InvalidName {
            name: name.to_string(),
            reason: "name must not end with '/'".to_string(),
        });
    }

    if name.contains("//") {
        return Err(ArtifactError::InvalidName {
            name: name.to_string(),
            reason: "name must not contain consecutive slashes".to_string(),
        });
    }

    if name.contains("../") || name.contains("/..") || name == ".." {
        return Err(ArtifactError::InvalidName {
            name: name.to_string(),
            reason: "name must not contain path traversal".to_string(),
        });
    }

    let valid = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/');
    if !valid {
        return Err(ArtifactError::InvalidName {
            name: name.to_string(),
            reason: "name contains invalid characters (allowed: alphanumeric, -, _, ., /)"
                .to_string(),
        });
    }

    Ok(())
}
