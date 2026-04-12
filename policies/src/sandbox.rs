//! Sandbox policy — restricts file paths to an allowed root directory.
#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use swink_agent::{PreDispatchPolicy, PreDispatchVerdict, ToolDispatchContext};

/// Rejects tool calls that reference file paths outside an allowed root directory.
///
/// Inspects string values in configured argument field names (default: `["path", "file_path", "file"]`).
/// Skips with a descriptive error on violation — does not silently rewrite paths.
///
/// # Example
/// ```rust,ignore
/// use swink_agent_policies::SandboxPolicy;
/// use swink_agent::AgentOptions;
///
/// let opts = AgentOptions::new(...)
///     .with_pre_dispatch_policy(SandboxPolicy::new("/tmp/workspace"));
/// ```
#[derive(Debug, Clone)]
pub struct SandboxPolicy {
    allowed_root: PathBuf,
    path_fields: Vec<String>,
}

impl SandboxPolicy {
    /// Create a new `SandboxPolicy` with the given allowed root.
    ///
    /// Default path fields: `["path", "file_path", "file"]`.
    pub fn new(allowed_root: impl Into<PathBuf>) -> Self {
        Self {
            allowed_root: allowed_root.into(),
            path_fields: vec![
                "path".to_string(),
                "file_path".to_string(),
                "file".to_string(),
            ],
        }
    }

    /// Override the argument field names to check for file paths.
    #[must_use]
    pub fn with_path_fields(mut self, fields: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.path_fields = fields.into_iter().map(Into::into).collect();
        self
    }

    fn validate_path(&self, path_str: &str, execution_root: Option<&Path>) -> Result<(), String> {
        let allowed_root = std::fs::canonicalize(&self.allowed_root).map_err(|err| {
            format!(
                "sandbox allowed root '{}' is unavailable: {err}",
                self.allowed_root.display()
            )
        })?;
        let resolved_path = self.resolve_path(Path::new(path_str), execution_root)?;

        if resolved_path.starts_with(&allowed_root) {
            Ok(())
        } else {
            Err(format!(
                "path '{}' resolves outside allowed root '{}'",
                resolved_path.display(),
                allowed_root.display()
            ))
        }
    }

    fn resolve_path(&self, path: &Path, execution_root: Option<&Path>) -> Result<PathBuf, String> {
        let _ = self; // Future use for per-instance resolution config.
        let base_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            let execution_root = execution_root.ok_or_else(|| {
                format!(
                    "relative path '{}' cannot be validated without an execution root",
                    path.display()
                )
            })?;
            let execution_root = std::fs::canonicalize(execution_root).map_err(|err| {
                format!(
                    "execution root '{}' is unavailable: {err}",
                    execution_root.display()
                )
            })?;
            execution_root.join(path)
        };

        Self::resolve_existing_prefix(&base_path)
    }

    fn resolve_existing_prefix(path: &Path) -> Result<PathBuf, String> {
        let mut unresolved_components: Vec<OsString> = Vec::new();
        let mut probe = path;

        loop {
            match std::fs::symlink_metadata(probe) {
                Ok(_) => {
                    let mut resolved = std::fs::canonicalize(probe).map_err(|err| {
                        format!("failed to canonicalize '{}': {err}", probe.display())
                    })?;
                    for component in unresolved_components.iter().rev() {
                        match Path::new(component).components().next() {
                            Some(Component::CurDir) => {}
                            Some(Component::ParentDir) => {
                                resolved.pop();
                            }
                            Some(Component::Normal(part)) => resolved.push(part),
                            Some(Component::RootDir | Component::Prefix(_)) | None => {
                                return Err(format!(
                                    "path '{}' contains an unsupported component",
                                    path.display()
                                ));
                            }
                        }
                    }
                    return Ok(resolved);
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    let component = probe.file_name().ok_or_else(|| {
                        format!(
                            "failed to resolve path '{}': no existing ancestor",
                            path.display()
                        )
                    })?;
                    unresolved_components.push(component.to_os_string());
                    probe = probe.parent().ok_or_else(|| {
                        format!(
                            "failed to resolve path '{}': no existing ancestor",
                            path.display()
                        )
                    })?;
                }
                Err(err) => {
                    return Err(format!(
                        "failed to inspect path '{}': {err}",
                        probe.display()
                    ));
                }
            }
        }
    }
}

impl PreDispatchPolicy for SandboxPolicy {
    fn name(&self) -> &'static str {
        "sandbox"
    }

    fn evaluate(&self, ctx: &mut ToolDispatchContext<'_>) -> PreDispatchVerdict {
        let Some(obj) = ctx.arguments.as_object() else {
            return PreDispatchVerdict::Continue;
        };

        for field_name in &self.path_fields {
            if let Some(serde_json::Value::String(path_str)) = obj.get(field_name.as_str())
                && let Err(reason) = self.validate_path(path_str, ctx.execution_root)
            {
                return PreDispatchVerdict::Skip(format!(
                    "path '{}' in field '{}' is outside allowed root '{}': {}",
                    path_str,
                    field_name,
                    self.allowed_root.display(),
                    reason
                ));
            }
        }

        PreDispatchVerdict::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_dispatch_ctx<'a>(
        tool_name: &'a str,
        args: &'a mut serde_json::Value,
        execution_root: Option<&'a Path>,
        state: &'a swink_agent::SessionState,
    ) -> ToolDispatchContext<'a> {
        ToolDispatchContext {
            tool_name,
            tool_call_id: "id1",
            arguments: args,
            execution_root,
            state,
        }
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
        assert!(
            matches!(result, PreDispatchVerdict::Skip(ref e) if e.contains("resolves outside"))
        );
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
        assert!(
            matches!(result, PreDispatchVerdict::Skip(ref e) if e.contains("resolves outside"))
        );
    }
}
