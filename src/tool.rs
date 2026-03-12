//! Tool system traits and validation for the swink agent.
//!
//! This module defines the [`AgentTool`] trait that all tools must implement,
//! the [`AgentToolResult`] type returned by tool execution, and the
//! [`validate_tool_arguments`] function for validating tool call arguments
//! against a JSON Schema.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use regex::Regex;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::types::ContentBlock;

// ─── AgentToolResult ─────────────────────────────────────────────────────────

/// The result of a tool execution.
///
/// Contains content blocks returned to the LLM and structured details for
/// logging that are not sent to the model.
#[derive(Debug, Clone)]
pub struct AgentToolResult {
    /// Content blocks returned to the LLM as the tool result.
    pub content: Vec<ContentBlock>,
    /// Structured data for logging and display; not sent to the LLM.
    pub details: Value,
}

impl AgentToolResult {
    /// Create a result containing a single text content block with null details.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::Text { text: text.into() }],
            details: Value::Null,
        }
    }

    /// Create an error result containing a single text content block with null
    /// details.
    ///
    /// Semantically identical to [`text`](Self::text) but communicates intent
    /// at the call site.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::Text {
                text: message.into(),
            }],
            details: Value::Null,
        }
    }
}

// ─── AgentTool Trait ─────────────────────────────────────────────────────────

/// A tool that can be invoked by the agent loop.
///
/// Implementations must be object-safe, `Send`, and `Sync`. The trait uses a
/// boxed future return type instead of `async fn` to maintain object safety.
pub trait AgentTool: Send + Sync {
    /// Unique routing key used to dispatch tool calls.
    fn name(&self) -> &str;

    /// Human-readable display name for logging and UI.
    fn label(&self) -> &str;

    /// Natural-language description included in the LLM prompt.
    fn description(&self) -> &str;

    /// JSON Schema describing the tool's input shape, used for validation.
    fn parameters_schema(&self) -> &Value;

    /// Whether this tool requires user approval before execution.
    /// Default is `false` — tools execute immediately.
    fn requires_approval(&self) -> bool {
        false
    }

    /// Execute the tool with validated parameters.
    ///
    /// # Arguments
    ///
    /// * `tool_call_id` — unique identifier for this particular invocation
    /// * `params` — validated input parameters as a JSON value
    /// * `cancellation_token` — token that signals the tool should abort
    /// * `on_update` — optional callback for streaming partial results
    fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
    ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>>;
}

// ─── Validation ──────────────────────────────────────────────────────────────

/// Validate tool call arguments against a JSON Schema.
///
/// Returns `Ok(())` when the arguments are valid, or `Err` with a list of
/// human-readable error strings describing each validation failure.
///
/// # Errors
///
/// Returns `Err(Vec<String>)` when the arguments fail schema validation,
/// containing one human-readable error string per violation.
pub fn validate_tool_arguments(schema: &Value, arguments: &Value) -> Result<(), Vec<String>> {
    let validator =
        jsonschema::validator_for(schema).map_err(|e| vec![format!("invalid schema: {e}")])?;

    let errors: Vec<String> = validator
        .iter_errors(arguments)
        .map(|e| e.to_string())
        .collect();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Build an error result for an unknown tool name.
#[must_use]
pub fn unknown_tool_result(tool_name: &str) -> AgentToolResult {
    AgentToolResult::error(format!("unknown tool: {tool_name}"))
}

/// Build an error result listing all validation errors.
#[must_use]
pub fn validation_error_result(errors: &[String]) -> AgentToolResult {
    let message = errors.join("\n");
    AgentToolResult::error(message)
}

// ─── Tool Approval ──────────────────────────────────────────────────────────

/// Result of the approval gate for a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolApproval {
    /// The tool call is approved and should proceed.
    Approved,
    /// The tool call is rejected and should not execute.
    Rejected,
}

/// Information about a tool call pending approval.
///
/// The [`Debug`] implementation redacts the `arguments` field to prevent
/// sensitive values from leaking into logs and debug output.
#[derive(Clone)]
pub struct ToolApprovalRequest {
    /// The unique ID of this tool call.
    pub tool_call_id: String,
    /// The name of the tool being called.
    pub tool_name: String,
    /// The arguments passed to the tool.
    pub arguments: Value,
    /// Whether the tool itself declared that it requires approval.
    pub requires_approval: bool,
}

impl fmt::Debug for ToolApprovalRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolApprovalRequest")
            .field("tool_call_id", &self.tool_call_id)
            .field("tool_name", &self.tool_name)
            .field("arguments", &"[REDACTED]")
            .field("requires_approval", &self.requires_approval)
            .finish()
    }
}

/// Controls whether the approval gate is active.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ApprovalMode {
    /// Every tool call goes through the approval callback.
    #[default]
    Enabled,
    /// Auto-approve read-only tools (where `requires_approval()` returns false);
    /// prompt for all others. Supports per-tool session trust.
    Smart,
    /// All tool calls auto-approved — callback is never called.
    /// Use this to temporarily disable approval without removing the callback.
    Bypassed,
}

// ─── selective_approve ───────────────────────────────────────────────────────

/// Wraps an approval callback so that only tools with `requires_approval == true`
/// go through the inner callback. All other tools are auto-approved.
#[allow(clippy::type_complexity)]
pub fn selective_approve<F>(
    inner: F,
) -> Box<
    dyn Fn(ToolApprovalRequest) -> Pin<Box<dyn Future<Output = ToolApproval> + Send>>
        + Send
        + Sync,
>
where
    F: Fn(ToolApprovalRequest) -> Pin<Box<dyn Future<Output = ToolApproval> + Send>>
        + Send
        + Sync
        + 'static,
{
    Box::new(move |req: ToolApprovalRequest| {
        if req.requires_approval {
            inner(req)
        } else {
            Box::pin(async { ToolApproval::Approved })
        }
    })
}

// ─── Sensitive Value Redaction ────────────────────────────────────────────────

/// Placeholder used to replace redacted values.
const REDACTED: &str = "[REDACTED]";

/// Key names whose values are always redacted, regardless of content.
const SENSITIVE_KEYS: &[&str] = &[
    "password",
    "secret",
    "token",
    "api_key",
    "apikey",
    "authorization",
];

/// Scan a [`serde_json::Value`] for common secret patterns and replace
/// matching string values with `"[REDACTED]"`.
///
/// Redaction rules:
/// - **Sensitive keys** — object keys named `password`, `secret`, `token`,
///   `api_key`, `apikey`, or `authorization` (case-insensitive) have their
///   values replaced unconditionally.
/// - **Value prefixes** — string values starting with `sk-`, `key-`, `token-`,
///   `bearer ` (case-insensitive), or `Basic ` (case-insensitive) are replaced.
/// - **Env-var patterns** — strings matching `$SECRET` or `${API_KEY}` style
///   references are replaced.
///
/// Non-string values and strings that do not match any pattern pass through
/// unchanged. The input value is cloned — the original is not modified.
#[must_use]
pub fn redact_sensitive_values(value: &Value) -> Value {
    redact_value(value, None)
}

/// Recursive redaction walker.
fn redact_value(value: &Value, parent_key: Option<&str>) -> Value {
    // If the parent key is sensitive, redact the entire value regardless of type.
    if let Some(key) = parent_key
        && SENSITIVE_KEYS
            .iter()
            .any(|&s| key.eq_ignore_ascii_case(s))
    {
        return Value::String(REDACTED.to_string());
    }

    match value {
        Value::String(s) => {
            if is_sensitive_string(s) {
                Value::String(REDACTED.to_string())
            } else {
                value.clone()
            }
        }
        Value::Array(arr) => {
            Value::Array(arr.iter().map(|v| redact_value(v, None)).collect())
        }
        Value::Object(map) => {
            let redacted = map
                .iter()
                .map(|(k, v)| (k.clone(), redact_value(v, Some(k))))
                .collect();
            Value::Object(redacted)
        }
        // Numbers, booleans, null — pass through.
        _ => value.clone(),
    }
}

/// Check whether a string value looks like a secret based on common prefixes
/// and environment-variable reference patterns.
fn is_sensitive_string(s: &str) -> bool {
    // Prefix patterns (case-sensitive for API key prefixes, insensitive for auth).
    if s.starts_with("sk-")
        || s.starts_with("key-")
        || s.starts_with("token-")
        || s.to_ascii_lowercase().starts_with("bearer ")
        || s.to_ascii_lowercase().starts_with("basic ")
    {
        return true;
    }

    // Env-var reference patterns: $SECRET or ${API_KEY}
    // Matches strings that are exactly an env-var reference.
    thread_local! {
        static ENV_VAR_RE: Regex =
            Regex::new(r"^\$\{?[A-Z_][A-Z0-9_]*\}?$").expect("valid regex");
    }
    ENV_VAR_RE.with(|re| re.is_match(s))
}

// ─── Compile-time Send + Sync assertions ────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AgentToolResult>();
    assert_send_sync::<ToolApproval>();
    assert_send_sync::<ToolApprovalRequest>();
    assert_send_sync::<ApprovalMode>();
};

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    // ─── ToolApprovalRequest Debug ──────────────────────────────────────────

    #[test]
    fn approval_request_debug_redacts_arguments() {
        let req = ToolApprovalRequest {
            tool_call_id: "call_1".into(),
            tool_name: "bash".into(),
            arguments: json!({"command": "echo secret"}),
            requires_approval: true,
        };
        let debug = format!("{req:?}");
        assert!(debug.contains("tool_call_id: \"call_1\""));
        assert!(debug.contains("tool_name: \"bash\""));
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("echo secret"));
    }

    // ─── redact_sensitive_values ────────────────────────────────────────────

    #[test]
    fn redacts_sk_prefix() {
        let val = json!({"key": "sk-abc123"});
        let redacted = redact_sensitive_values(&val);
        assert_eq!(redacted["key"], json!("[REDACTED]"));
    }

    #[test]
    fn redacts_key_prefix() {
        let val = json!({"data": "key-live-xyz"});
        let redacted = redact_sensitive_values(&val);
        assert_eq!(redacted["data"], json!("[REDACTED]"));
    }

    #[test]
    fn redacts_token_prefix() {
        let val = json!({"tok": "token-abcdef"});
        let redacted = redact_sensitive_values(&val);
        assert_eq!(redacted["tok"], json!("[REDACTED]"));
    }

    #[test]
    fn redacts_bearer_prefix_case_insensitive() {
        let val = json!({"auth": "Bearer eyJhbGciOi..."});
        let redacted = redact_sensitive_values(&val);
        assert_eq!(redacted["auth"], json!("[REDACTED]"));

        let val2 = json!({"auth": "bearer xyz"});
        let redacted2 = redact_sensitive_values(&val2);
        assert_eq!(redacted2["auth"], json!("[REDACTED]"));
    }

    #[test]
    fn redacts_basic_prefix_case_insensitive() {
        let val = json!({"auth": "Basic dXNlcjpwYXNz"});
        let redacted = redact_sensitive_values(&val);
        assert_eq!(redacted["auth"], json!("[REDACTED]"));

        let val2 = json!({"auth": "basic abc"});
        let redacted2 = redact_sensitive_values(&val2);
        assert_eq!(redacted2["auth"], json!("[REDACTED]"));
    }

    #[test]
    fn redacts_env_var_dollar_sign() {
        let val = json!({"ref": "$SECRET"});
        let redacted = redact_sensitive_values(&val);
        assert_eq!(redacted["ref"], json!("[REDACTED]"));
    }

    #[test]
    fn redacts_env_var_braced() {
        let val = json!({"ref": "${API_KEY}"});
        let redacted = redact_sensitive_values(&val);
        assert_eq!(redacted["ref"], json!("[REDACTED]"));
    }

    #[test]
    fn redacts_sensitive_key_password() {
        let val = json!({"password": "hunter2"});
        let redacted = redact_sensitive_values(&val);
        assert_eq!(redacted["password"], json!("[REDACTED]"));
    }

    #[test]
    fn redacts_sensitive_key_secret() {
        let val = json!({"secret": "mysecret"});
        let redacted = redact_sensitive_values(&val);
        assert_eq!(redacted["secret"], json!("[REDACTED]"));
    }

    #[test]
    fn redacts_sensitive_key_token() {
        let val = json!({"Token": "abc"});
        let redacted = redact_sensitive_values(&val);
        assert_eq!(redacted["Token"], json!("[REDACTED]"));
    }

    #[test]
    fn redacts_sensitive_key_api_key() {
        let val = json!({"api_key": "abc"});
        let redacted = redact_sensitive_values(&val);
        assert_eq!(redacted["api_key"], json!("[REDACTED]"));
    }

    #[test]
    fn redacts_sensitive_key_apikey() {
        let val = json!({"apiKey": "abc"});
        let redacted = redact_sensitive_values(&val);
        assert_eq!(redacted["apiKey"], json!("[REDACTED]"));
    }

    #[test]
    fn redacts_sensitive_key_authorization() {
        let val = json!({"Authorization": "something"});
        let redacted = redact_sensitive_values(&val);
        assert_eq!(redacted["Authorization"], json!("[REDACTED]"));
    }

    #[test]
    fn passes_through_non_sensitive_values() {
        let val = json!({
            "command": "echo hello",
            "path": "/tmp/file.txt",
            "count": 42,
            "verbose": true,
            "items": ["one", "two"]
        });
        let redacted = redact_sensitive_values(&val);
        assert_eq!(redacted, val);
    }

    #[test]
    fn redacts_nested_objects() {
        let val = json!({
            "config": {
                "password": "secret123",
                "host": "localhost"
            }
        });
        let redacted = redact_sensitive_values(&val);
        assert_eq!(redacted["config"]["password"], json!("[REDACTED]"));
        assert_eq!(redacted["config"]["host"], json!("localhost"));
    }

    #[test]
    fn redacts_values_in_arrays() {
        let val = json!(["normal", "sk-secret", "also normal"]);
        let redacted = redact_sensitive_values(&val);
        assert_eq!(
            redacted,
            json!(["normal", "[REDACTED]", "also normal"])
        );
    }

    #[test]
    fn handles_null_and_numbers() {
        let val = json!({"a": null, "b": 42, "c": 2.72});
        let redacted = redact_sensitive_values(&val);
        assert_eq!(redacted, val);
    }

    // ─── ApprovalMode ─────────────────────────────────────────────────────

    #[test]
    fn approval_mode_default_is_enabled() {
        assert_eq!(ApprovalMode::default(), ApprovalMode::Enabled);
    }

    #[test]
    fn approval_mode_variants_are_distinct() {
        assert_ne!(ApprovalMode::Enabled, ApprovalMode::Smart);
        assert_ne!(ApprovalMode::Smart, ApprovalMode::Bypassed);
        assert_ne!(ApprovalMode::Enabled, ApprovalMode::Bypassed);
    }
}
