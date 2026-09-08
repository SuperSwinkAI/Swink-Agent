use swink_agent::ArtifactError;

/// Validate an artifact name.
///
/// Allowed characters: alphanumeric, hyphens, underscores, dots, forward slashes.
/// Must not be empty, start/end with `/`, or contain `//` or `../`.
pub fn validate_artifact_name(name: &str) -> Result<(), ArtifactError> {
    swink_agent::validate_artifact_name(name)
}

#[cfg(test)]
#[path = "validate_tests.rs"]
mod tests;
