use swink_agent::ArtifactError;

/// Validate an artifact name.
///
/// Allowed characters: alphanumeric, hyphens, underscores, dots, forward slashes.
/// Must not be empty, start/end with `/`, or contain `//` or `../`.
pub fn validate_artifact_name(name: &str) -> Result<(), ArtifactError> {
    swink_agent::validate_artifact_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_simple_name() {
        assert!(validate_artifact_name("report.md").is_ok());
    }

    #[test]
    fn valid_name_with_hyphens_and_underscores() {
        assert!(validate_artifact_name("my-report_v2.txt").is_ok());
    }

    #[test]
    fn valid_name_with_path() {
        assert!(validate_artifact_name("tools/output/data.csv").is_ok());
    }

    #[test]
    fn valid_alphanumeric_only() {
        assert!(validate_artifact_name("report2024").is_ok());
    }

    #[test]
    fn valid_single_character() {
        assert!(validate_artifact_name("a").is_ok());
    }

    #[test]
    fn valid_dotfile() {
        assert!(validate_artifact_name(".gitignore").is_ok());
    }

    #[test]
    fn invalid_empty_name() {
        let err = validate_artifact_name("").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("empty"), "expected 'empty' in: {msg}");
    }

    #[test]
    fn invalid_leading_slash() {
        let err = validate_artifact_name("/report.md").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("start with '/'"),
            "expected start-with-slash message in: {msg}"
        );
    }

    #[test]
    fn invalid_trailing_slash() {
        let err = validate_artifact_name("report/").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("end with '/'"),
            "expected end-with-slash message in: {msg}"
        );
    }

    #[test]
    fn invalid_consecutive_slashes() {
        let err = validate_artifact_name("tools//output.txt").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("consecutive slashes"),
            "expected consecutive-slashes message in: {msg}"
        );
    }

    #[test]
    fn invalid_path_traversal_prefix() {
        let err = validate_artifact_name("../etc/passwd").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("path traversal"),
            "expected path-traversal message in: {msg}"
        );
    }

    #[test]
    fn invalid_path_traversal_mid_path() {
        let err = validate_artifact_name("tools/../secret.txt").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("path traversal"),
            "expected path-traversal message in: {msg}"
        );
    }

    #[test]
    fn invalid_path_traversal_suffix() {
        let err = validate_artifact_name("tools/..").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("path traversal"),
            "expected path-traversal message in: {msg}"
        );
    }

    #[test]
    fn invalid_path_traversal_bare() {
        let err = validate_artifact_name("..").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("path traversal"),
            "expected path-traversal message in: {msg}"
        );
    }

    #[test]
    fn invalid_special_characters() {
        for ch in ['@', '#', '$', '%', '!', '?', '*', '~', '&'] {
            let name = format!("report{ch}file.md");
            let err = validate_artifact_name(&name).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("invalid character"),
                "expected invalid-character message for '{ch}' in: {msg}"
            );
        }
    }

    #[test]
    fn invalid_space_in_name() {
        let err = validate_artifact_name("my report.md").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("invalid character"),
            "expected invalid-character message in: {msg}"
        );
    }

    #[test]
    fn invalid_backslash() {
        let err = validate_artifact_name("tools\\output.txt").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("invalid character"),
            "expected invalid-character message in: {msg}"
        );
    }
}
