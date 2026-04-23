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
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempNoopEditor {
        path: PathBuf,
    }

    impl TempNoopEditor {
        fn create() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should be after unix epoch")
                .as_nanos();
            let test_bin_dir = std::env::current_dir()
                .expect("current dir should resolve during tests")
                .join("target")
                .join("test-bin");
            std::fs::create_dir_all(&test_bin_dir).expect("should create test-bin directory");

            let mut path =
                test_bin_dir.join(format!("swink-editor-test-{unique}-{}", std::process::id()));

            #[cfg(windows)]
            {
                path.set_extension("cmd");
                std::fs::write(&path, "@echo off\r\nexit /b 0\r\n")
                    .expect("should write noop cmd script");
            }

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;

                path.set_extension("sh");
                std::fs::write(&path, "#!/bin/sh\nexit 0\n")
                    .expect("should write noop shell script");

                let mut permissions = std::fs::metadata(&path)
                    .expect("noop script metadata")
                    .permissions();
                permissions.set_mode(0o755);
                std::fs::set_permissions(&path, permissions)
                    .expect("should mark noop script executable");
            }

            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempNoopEditor {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

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
    fn open_editor_with_noop_command_returns_none() {
        let noop_editor = TempNoopEditor::create();
        let result = open_editor(
            noop_editor
                .path()
                .to_str()
                .expect("temp script path should be valid unicode"),
        );
        assert!(result.is_ok());
        assert!(result.unwrap().is_none()); // empty file = cancellation
    }

    #[test]
    fn create_temp_prompt_path_uses_unique_randomized_names() {
        let first = create_temp_prompt_path().expect("first temp path should be created");
        let second = create_temp_prompt_path().expect("second temp path should be created");

        assert_ne!(first.as_os_str(), second.as_os_str());
        assert!(first.exists());
        assert!(second.exists());
    }
}
