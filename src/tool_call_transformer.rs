//! Pre-execution tool-call argument transformation hook.
//!
//! [`ToolCallTransformer`] runs after approval but before validation, allowing
//! programmatic argument rewriting (sandboxing, path rewrites, etc.) without
//! affecting the approval flow.

use serde_json::Value;

/// Transforms tool-call arguments before validation and execution.
///
/// Runs unconditionally (not gated by approval mode). Use this for
/// programmatic rewrites such as sandboxing paths, injecting defaults,
/// or normalizing arguments.
///
/// # Execution order
///
/// Approval → **Transformer** → Validator → Schema validation → `execute()`
pub trait ToolCallTransformer: Send + Sync {
    /// Mutate `arguments` in place for the given `tool_name`.
    fn transform(&self, tool_name: &str, arguments: &mut Value);
}

/// Blanket impl for closures matching the transformer signature.
impl<F: Fn(&str, &mut Value) + Send + Sync> ToolCallTransformer for F {
    fn transform(&self, tool_name: &str, arguments: &mut Value) {
        self(tool_name, arguments);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn closure_as_transformer() {
        let transformer = |_name: &str, args: &mut Value| {
            args["injected"] = json!(true);
        };
        let mut args = json!({"path": "/tmp/foo"});
        transformer.transform("bash", &mut args);
        assert_eq!(args["injected"], json!(true));
        assert_eq!(args["path"], json!("/tmp/foo"));
    }

    #[test]
    fn transformer_can_rewrite_arguments() {
        let transformer = |name: &str, args: &mut Value| {
            if name == "bash"
                && let Some(cmd) = args.get("command").and_then(Value::as_str)
            {
                args["command"] = Value::String(format!("sandbox {cmd}"));
            }
        };
        let mut args = json!({"command": "ls /"});
        transformer.transform("bash", &mut args);
        assert_eq!(args["command"], json!("sandbox ls /"));
    }

    #[test]
    fn transformer_noop_for_unknown_tool() {
        let transformer = |name: &str, args: &mut Value| {
            if name == "bash" {
                args["sandboxed"] = json!(true);
            }
        };
        let mut args = json!({"path": "/foo"});
        transformer.transform("read_file", &mut args);
        assert!(args.get("sandboxed").is_none());
    }
}
