//! Tests for `editor`.
#![cfg(test)]

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
            std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("should write noop shell script");

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
    let command = noop_editor
        .path()
        .to_str()
        .expect("temp script path should be valid unicode");
    // The script was written moments ago; a sibling test that forks in
    // between can still hold the write fd open in its child, and exec
    // then fails with ETXTBSY. Retry until the fd is gone.
    let mut result = open_editor(command);
    for _ in 0..50 {
        match &result {
            Err(e) if e.kind() == io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(std::time::Duration::from_millis(10));
                result = open_editor(command);
            }
            _ => break,
        }
    }
    assert!(result.is_ok(), "{result:?}");
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
