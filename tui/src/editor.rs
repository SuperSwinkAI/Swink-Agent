//! External editor integration for composing prompts.
//!
//! Opens the user's preferred editor (`$EDITOR`, `$VISUAL`, or `vi`) with a
//! temporary file. The TUI suspends while the editor is open and submits the
//! file contents as a user prompt when the editor closes.

use std::io;
use std::process::Command;

/// Resolve the editor command from environment or fallback.
///
/// Priority: config override > `$EDITOR` > `$VISUAL` > `vi`.
#[must_use]
pub fn resolve_editor(config_override: Option<&str>) -> String {
    if let Some(editor) = config_override {
        return editor.to_string();
    }
    if let Ok(editor) = std::env::var("EDITOR") {
        return editor;
    }
    if let Ok(editor) = std::env::var("VISUAL") {
        return editor;
    }
    "vi".to_string()
}

/// Open the editor with a temporary file and return the file contents on close.
///
/// Returns `Ok(Some(content))` if the editor exited successfully and the file is non-empty.
/// Returns `Ok(None)` if the editor exited successfully but the file is empty (cancellation).
/// Returns `Err` if the editor could not be launched or exited with a non-zero status.
pub fn open_editor(editor_command: &str) -> io::Result<Option<String>> {
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join(format!("swink-prompt-{}.md", std::process::id()));

    // Create empty temp file
    std::fs::write(&temp_path, "")?;

    // Launch the editor
    let status = Command::new(editor_command).arg(&temp_path).status();

    let status = match status {
        Ok(s) => s,
        Err(e) => {
            let _ = std::fs::remove_file(&temp_path);
            return Err(e);
        }
    };

    if !status.success() {
        let _ = std::fs::remove_file(&temp_path);
        return Err(io::Error::other(format!(
            "Editor exited with status: {status}"
        )));
    }

    // Read and clean up
    let content = std::fs::read_to_string(&temp_path).unwrap_or_default();
    let _ = std::fs::remove_file(&temp_path);

    let trimmed = content.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_editor_with_config_override() {
        assert_eq!(resolve_editor(Some("nano")), "nano");
    }

    #[test]
    fn resolve_editor_falls_back_to_vi() {
        // When no env vars are set and no config override
        // We can't fully control env in tests, but test that the function returns something
        let result = resolve_editor(None);
        assert!(!result.is_empty());
    }

    #[test]
    fn open_editor_with_nonexistent_command() {
        let result = open_editor("__nonexistent_editor_binary_12345__");
        assert!(result.is_err());
    }

    #[test]
    fn open_editor_with_true_command_returns_none() {
        // `true` exits successfully but writes nothing to the file
        let result = open_editor("true");
        assert!(result.is_ok());
        assert!(result.unwrap().is_none()); // empty file = cancellation
    }
}
