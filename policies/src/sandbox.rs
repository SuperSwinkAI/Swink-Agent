//! Sandbox policy — restricts file paths to an allowed root directory.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use swink_agent::{PolicyContext, PreDispatchPolicy, PreDispatchVerdict, ToolPolicyContext};

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
    pub fn with_path_fields(
        mut self,
        fields: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.path_fields = fields.into_iter().map(Into::into).collect();
        self
    }

    /// Check if a path is within the allowed root.
    fn is_path_allowed(&self, path_str: &str) -> bool {
        let path = Path::new(path_str);

        // Reject paths containing `..` traversal
        for component in path.components() {
            if matches!(component, std::path::Component::ParentDir) {
                return false;
            }
        }

        // Check if the path starts with the allowed root
        // For relative paths, they're allowed (they'll resolve within the working dir)
        if path.is_absolute() {
            path.starts_with(&self.allowed_root)
        } else {
            // Relative paths are allowed as long as they don't contain ..
            true
        }
    }
}

impl PreDispatchPolicy for SandboxPolicy {
    fn name(&self) -> &'static str {
        "sandbox"
    }

    fn evaluate(
        &self,
        _ctx: &PolicyContext<'_>,
        tool: &mut ToolPolicyContext<'_>,
    ) -> PreDispatchVerdict {
        let Some(obj) = tool.arguments.as_object() else {
            return PreDispatchVerdict::Continue;
        };

        for field_name in &self.path_fields {
            if let Some(serde_json::Value::String(path_str)) = obj.get(field_name.as_str())
                && !self.is_path_allowed(path_str) {
                    return PreDispatchVerdict::Skip(format!(
                        "path '{}' in field '{}' is outside allowed root '{}'",
                        path_str,
                        field_name,
                        self.allowed_root.display()
                    ));
                }
        }

        PreDispatchVerdict::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swink_agent::{Cost, Usage};

    fn make_ctx<'a>(usage: &'a Usage, cost: &'a Cost) -> PolicyContext<'a> {
        PolicyContext {
            turn_index: 0,
            accumulated_usage: usage,
            accumulated_cost: cost,
            message_count: 0,
            overflow_signal: false,
            new_messages: &[],
        }
    }

    #[test]
    fn rejects_path_outside_root() {
        let policy = SandboxPolicy::new("/tmp/workspace");
        let usage = Usage::default();
        let cost = Cost::default();
        let ctx = make_ctx(&usage, &cost);
        let mut args = serde_json::json!({"path": "/etc/passwd"});
        let mut tool_ctx = ToolPolicyContext {
            tool_name: "write_file",
            tool_call_id: "id1",
            arguments: &mut args,
        };
        let result = policy.evaluate(&ctx, &mut tool_ctx);
        assert!(matches!(result, PreDispatchVerdict::Skip(ref e) if e.contains("/etc/passwd")));
    }

    #[test]
    fn allows_path_inside_root() {
        let policy = SandboxPolicy::new("/tmp/workspace");
        let usage = Usage::default();
        let cost = Cost::default();
        let ctx = make_ctx(&usage, &cost);
        let mut args = serde_json::json!({"path": "/tmp/workspace/output.txt"});
        let mut tool_ctx = ToolPolicyContext {
            tool_name: "write_file",
            tool_call_id: "id1",
            arguments: &mut args,
        };
        let result = policy.evaluate(&ctx, &mut tool_ctx);
        assert!(matches!(result, PreDispatchVerdict::Continue));
    }

    #[test]
    fn handles_path_traversal_attack() {
        let policy = SandboxPolicy::new("/tmp/workspace");
        let usage = Usage::default();
        let cost = Cost::default();
        let ctx = make_ctx(&usage, &cost);
        let mut args = serde_json::json!({"path": "/tmp/workspace/../../etc/passwd"});
        let mut tool_ctx = ToolPolicyContext {
            tool_name: "write_file",
            tool_call_id: "id1",
            arguments: &mut args,
        };
        let result = policy.evaluate(&ctx, &mut tool_ctx);
        assert!(matches!(result, PreDispatchVerdict::Skip(_)));
    }

    #[test]
    fn only_checks_configured_fields() {
        let policy = SandboxPolicy::new("/tmp/workspace");
        let usage = Usage::default();
        let cost = Cost::default();
        let ctx = make_ctx(&usage, &cost);
        // "command" is not in the default path_fields, so it won't be checked
        let mut args = serde_json::json!({"command": "/etc/passwd"});
        let mut tool_ctx = ToolPolicyContext {
            tool_name: "bash",
            tool_call_id: "id1",
            arguments: &mut args,
        };
        let result = policy.evaluate(&ctx, &mut tool_ctx);
        assert!(matches!(result, PreDispatchVerdict::Continue));
    }

    #[test]
    fn custom_path_fields() {
        let policy = SandboxPolicy::new("/tmp/workspace")
            .with_path_fields(["target_dir", "output"]);
        let usage = Usage::default();
        let cost = Cost::default();
        let ctx = make_ctx(&usage, &cost);
        let mut args = serde_json::json!({"target_dir": "/etc/shadow"});
        let mut tool_ctx = ToolPolicyContext {
            tool_name: "deploy",
            tool_call_id: "id1",
            arguments: &mut args,
        };
        let result = policy.evaluate(&ctx, &mut tool_ctx);
        assert!(matches!(result, PreDispatchVerdict::Skip(_)));
    }
}
