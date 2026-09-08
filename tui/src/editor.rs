//! External editor integration for composing prompts.
//!
//! Opens the user's preferred editor (`$EDITOR`, `$VISUAL`, or `vi`) with a
//! temporary file. The TUI suspends while the editor is open and submits the
//! file contents as a user prompt when the editor closes.

use std::io;
use std::process::Command;

use tempfile::NamedTempFile;

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
    let temp_path = create_temp_prompt_path()?;

    // Launch the editor
    let status = Command::new(editor_command).arg(&temp_path).status()?;

    if !status.success() {
        return Err(io::Error::other(format!(
            "Editor exited with status: {status}"
        )));
    }

    // Read the file before dropping the temp path so the randomized file is
    // still available to the spawned editor across platforms.
    let content = std::fs::read_to_string(&temp_path).unwrap_or_default();

    let trimmed = content.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}

fn create_temp_prompt_path() -> io::Result<tempfile::TempPath> {
    Ok(NamedTempFile::new()?.into_temp_path())
}

#[cfg(test)]
#[path = "editor_tests.rs"]
mod tests;
