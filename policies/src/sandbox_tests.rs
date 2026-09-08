//! Tests for `sandbox`.
#![cfg(test)]

use super::*;
use tempfile::TempDir;

fn make_dispatch_ctx<'a>(
    tool_name: &'a str,
    args: &'a mut serde_json::Value,
    execution_root: Option<&'a Path>,
    state: &'a swink_agent::SessionState,
) -> ToolDispatchContext<'a> {
    ToolDispatchContext::new(tool_name, "id1", args, execution_root, state)
}

fn sandbox_fixture() -> (TempDir, PathBuf) {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let allowed_root = tempdir.path().join("workspace");
    std::fs::create_dir_all(&allowed_root).expect("workspace");
    (tempdir, allowed_root)
}

#[test]
fn rejects_path_outside_root() {
    let (_tempdir, allowed_root) = sandbox_fixture();
    let policy = SandboxPolicy::new(&allowed_root);
    let state = swink_agent::SessionState::new();
    let outside = allowed_root.parent().unwrap().join("outside.txt");
    let mut args = serde_json::json!({"path": outside});
    let mut ctx = make_dispatch_ctx("write_file", &mut args, None, &state);
    let result = policy.evaluate(&mut ctx);
    assert!(matches!(result, PreDispatchVerdict::Skip(ref e) if e.contains("outside")));
}

#[test]
fn allows_path_inside_root() {
    let (_tempdir, allowed_root) = sandbox_fixture();
    let policy = SandboxPolicy::new(&allowed_root);
    let state = swink_agent::SessionState::new();
    let mut args = serde_json::json!({"path": allowed_root.join("output.txt")});
    let mut ctx = make_dispatch_ctx("write_file", &mut args, None, &state);
    let result = policy.evaluate(&mut ctx);
    assert!(matches!(result, PreDispatchVerdict::Continue));
}

#[test]
fn handles_path_traversal_attack() {
    let (_tempdir, allowed_root) = sandbox_fixture();
    let policy = SandboxPolicy::new(&allowed_root);
    let state = swink_agent::SessionState::new();
    let mut args = serde_json::json!({"path": allowed_root.join("../outside/passwd")});
    let mut ctx = make_dispatch_ctx("write_file", &mut args, None, &state);
    let result = policy.evaluate(&mut ctx);
    assert!(matches!(result, PreDispatchVerdict::Skip(_)));
}

#[test]
fn only_checks_configured_fields() {
    let (_tempdir, allowed_root) = sandbox_fixture();
    let policy = SandboxPolicy::new(&allowed_root);
    let state = swink_agent::SessionState::new();
    // "command" is not in the default path_fields, so it won't be checked
    let mut args = serde_json::json!({"command": "/etc/passwd"});
    let mut ctx = make_dispatch_ctx("bash", &mut args, None, &state);
    let result = policy.evaluate(&mut ctx);
    assert!(matches!(result, PreDispatchVerdict::Continue));
}

#[test]
fn custom_path_fields() {
    let (_tempdir, allowed_root) = sandbox_fixture();
    let policy = SandboxPolicy::new(&allowed_root).with_path_fields(["target_dir", "output"]);
    let state = swink_agent::SessionState::new();
    let mut args = serde_json::json!({"target_dir": allowed_root.join("../shadow")});
    let mut ctx = make_dispatch_ctx("deploy", &mut args, None, &state);
    let result = policy.evaluate(&mut ctx);
    assert!(matches!(result, PreDispatchVerdict::Skip(_)));
}

#[test]
fn rejects_relative_path_outside_allowed_root() {
    let (_tempdir, allowed_root) = sandbox_fixture();
    let execution_root = allowed_root.parent().unwrap().join("different-cwd");
    std::fs::create_dir_all(&execution_root).expect("execution root");
    let policy = SandboxPolicy::new(&allowed_root);
    let state = swink_agent::SessionState::new();
    let mut args = serde_json::json!({"path": "output.txt"});
    let mut ctx = make_dispatch_ctx("write_file", &mut args, Some(&execution_root), &state);
    let result = policy.evaluate(&mut ctx);
    assert!(matches!(result, PreDispatchVerdict::Skip(ref e) if e.contains("resolves outside")));
}

#[test]
fn allows_relative_path_inside_allowed_root() {
    let (_tempdir, allowed_root) = sandbox_fixture();
    let execution_root = allowed_root.join("nested");
    std::fs::create_dir_all(&execution_root).expect("execution root");
    let policy = SandboxPolicy::new(&allowed_root);
    let state = swink_agent::SessionState::new();
    let mut args = serde_json::json!({"path": "output.txt"});
    let mut ctx = make_dispatch_ctx("write_file", &mut args, Some(&execution_root), &state);
    let result = policy.evaluate(&mut ctx);
    assert!(matches!(result, PreDispatchVerdict::Continue));
}

#[test]
fn rejects_relative_path_without_execution_root() {
    let (_tempdir, allowed_root) = sandbox_fixture();
    let policy = SandboxPolicy::new(&allowed_root);
    let state = swink_agent::SessionState::new();
    let mut args = serde_json::json!({"path": "output.txt"});
    let mut ctx = make_dispatch_ctx("write_file", &mut args, None, &state);
    let result = policy.evaluate(&mut ctx);
    assert!(
        matches!(result, PreDispatchVerdict::Skip(ref e) if e.contains("cannot be validated without an execution root"))
    );
}

#[cfg(unix)]
#[test]
fn rejects_symlink_escape_inside_allowed_root() {
    let (_tempdir, allowed_root) = sandbox_fixture();
    let outside = allowed_root.parent().unwrap().join("outside-dir");
    std::fs::create_dir_all(&outside).expect("outside dir");
    let link = allowed_root.join("escape-link");
    std::os::unix::fs::symlink(&outside, &link).expect("symlink");

    let policy = SandboxPolicy::new(&allowed_root);
    let state = swink_agent::SessionState::new();
    let mut args = serde_json::json!({"path": link.join("secret.txt")});
    let mut ctx = make_dispatch_ctx("write_file", &mut args, None, &state);
    let result = policy.evaluate(&mut ctx);
    assert!(matches!(result, PreDispatchVerdict::Skip(ref e) if e.contains("resolves outside")));
}
