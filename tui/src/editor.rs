//! External editor integration for composing prompts.
//!
//! Opens the user's preferred editor (`$EDITOR`, `$VISUAL`, or `vi`) with a
//! temporary file. The TUI suspends while the editor is open and submits the
//! file contents as a user prompt when the editor closes.

use std::io;
use std::path::Path;
use std::process::{Command, ExitStatus};

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
/// Uses `tempfile::Builder` to create an exclusive, randomized temp path so
/// the file cannot collide with another user's file in a shared temp
/// directory. The temp file is removed when the returned `NamedTempFile`
/// goes out of scope, which happens before this function returns regardless
/// of success or failure.
///
/// Returns `Ok(Some(content))` if the editor exited successfully and the file is non-empty.
/// Returns `Ok(None)` if the editor exited successfully but the file is empty (cancellation).
/// Returns `Err` if the editor could not be launched or exited with a non-zero status.
pub fn open_editor(editor_command: &str) -> io::Result<Option<String>> {
    open_editor_with(editor_command, default_spawn)
}

/// Default spawn strategy: actually launch the editor and wait for it.
fn default_spawn(editor_command: &str, path: &Path) -> io::Result<ExitStatus> {
    Command::new(editor_command).arg(path).status()
}

/// Core editor helper with an injectable spawn function so tests can
/// observe the temp path (and verify it is randomized) without launching
/// a real editor.
fn open_editor_with<F>(editor_command: &str, spawn: F) -> io::Result<Option<String>>
where
    F: FnOnce(&str, &Path) -> io::Result<ExitStatus>,
{
    // `tempfile::Builder` creates a file with a randomized, exclusive path
    // (O_CREAT | O_EXCL on Unix, equivalent on Windows) so there is no
    // predictable filename to race against.
    let temp = tempfile::Builder::new()
        .prefix("swink-prompt-")
        .suffix(".md")
        .rand_bytes(16)
        .tempfile()?;

    let temp_path = temp.path().to_path_buf();

    // Launch the editor. If it fails to spawn, `temp` is dropped here and
    // the file is cleaned up automatically.
    let status = spawn(editor_command, &temp_path)?;

    if !status.success() {
        return Err(io::Error::other(format!(
            "Editor exited with status: {status}"
        )));
    }

    // Read the edited content. Reading via the path keeps this compatible
    // with editors that replace the file (write-to-temp-then-rename) rather
    // than editing in-place.
    let content = std::fs::read_to_string(&temp_path).unwrap_or_default();
    // `temp` is dropped at end of scope, deleting the file.

    let trimmed = content.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::sync::{Arc, Mutex};

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

    /// Produce a real success `ExitStatus` without relying on platform-specific
    /// `ExitStatusExt`. Using `Command::new("true")` gives us a cross-platform
    /// success status on any host that has a `true` binary (all CI targets
    /// for this crate: Linux, macOS; Windows is skipped via `cfg(unix)` below).
    #[cfg(unix)]
    fn success_status() -> ExitStatus {
        Command::new("true")
            .status()
            .expect("`true` command should succeed")
    }

    #[cfg(unix)]
    #[test]
    fn temp_path_is_randomized_and_cleaned_up() {
        // Run the helper twice with a stub spawner that captures the temp
        // path used. The paths must differ (random suffix) and the files
        // must be removed after `open_editor_with` returns.
        let captured: Arc<Mutex<Vec<std::path::PathBuf>>> = Arc::new(Mutex::new(Vec::new()));

        for _ in 0..2 {
            let captured = Arc::clone(&captured);
            let result = open_editor_with("unused", move |_cmd, path| {
                // Stub editor: the temp file must exist when the editor
                // would be launched.
                assert!(
                    path.exists(),
                    "temp file must exist for spawned editor, path={}",
                    path.display()
                );
                captured.lock().unwrap().push(path.to_path_buf());
                Ok(success_status())
            })
            .expect("open_editor_with should succeed");
            // Empty file -> Ok(None)
            assert!(result.is_none());
        }

        let paths = captured.lock().unwrap().clone();
        assert_eq!(paths.len(), 2);

        // Neither path is the old predictable `swink-prompt-<pid>.md` form.
        let predictable = format!("swink-prompt-{}.md", std::process::id());
        for p in &paths {
            let name = p
                .file_name()
                .expect("temp path has a filename")
                .to_string_lossy()
                .to_string();
            assert_ne!(
                name, predictable,
                "temp filename must not be the old predictable form"
            );
            // It should still start with our prefix and end with the suffix.
            assert!(
                name.starts_with("swink-prompt-") && name.ends_with(".md"),
                "unexpected temp filename: {name}"
            );
            // And the random middle must be non-trivial (>= a few chars).
            let middle = &name["swink-prompt-".len()..name.len() - ".md".len()];
            assert!(
                middle.len() >= 8,
                "random segment too short to be exclusive: {middle:?}"
            );
        }

        // Two successive calls must produce distinct paths (random suffix).
        assert_ne!(
            paths[0], paths[1],
            "two temp files must have distinct randomized paths"
        );

        // Both files must have been cleaned up when open_editor_with returned.
        for p in &paths {
            assert!(
                !p.exists(),
                "temp file should be deleted after open_editor_with returns: {}",
                p.display()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_zero_exit_status_is_error_and_cleans_up() {
        let captured: Arc<Mutex<Option<std::path::PathBuf>>> = Arc::new(Mutex::new(None));
        let captured_clone = Arc::clone(&captured);
        let result = open_editor_with("unused", move |_cmd, path| {
            captured_clone.lock().unwrap().replace(path.to_path_buf());
            // `false` is a tiny portable binary that exits non-zero on Unix.
            Command::new("false").status()
        });
        assert!(result.is_err());
        let path = captured.lock().unwrap().clone().expect("path captured");
        assert!(
            !path.exists(),
            "temp file should be cleaned up after non-zero exit"
        );
    }
}
