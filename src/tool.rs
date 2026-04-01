//! Tool system traits and validation for the swink agent.
//!
//! This module defines the [`AgentTool`] trait that all tools must implement,
//! the [`AgentToolResult`] type returned by tool execution, and the
//! [`validate_tool_arguments`] function for validating tool call arguments
//! against a JSON Schema.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::{LazyLock, Mutex};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::agent_options::{ApproveToolFn, ApproveToolFuture};
use crate::types::ContentBlock;

static SCHEMA_VALIDATOR_CACHE: LazyLock<Mutex<HashMap<String, Arc<jsonschema::Validator>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// ─── AgentToolResult ─────────────────────────────────────────────────────────

/// The result of a tool execution.
///
/// Contains content blocks returned to the LLM and structured details for
/// logging that are not sent to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolResult {
    /// Content blocks returned to the LLM as the tool result.
    pub content: Vec<ContentBlock>,
    /// Structured data for logging and display; not sent to the LLM.
    pub details: Value,
    /// Whether this result represents an error condition.
    pub is_error: bool,
}

impl AgentToolResult {
    /// Create a result containing a single text content block with null details.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::Text { text: text.into() }],
            details: Value::Null,
            is_error: false,
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
            is_error: true,
        }
    }
}

/// A boxed future returned by [`AgentTool`] execution.
pub type ToolFuture<'a> = Pin<Box<dyn Future<Output = AgentToolResult> + Send + 'a>>;

// ─── Tool Metadata ──────────────────────────────────────────────────────────

/// Optional organizational metadata for an [`AgentTool`].
///
/// Groups tools by namespace and tracks version. Existing tools default to
/// no namespace and no version.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolMetadata {
    /// Logical grouping such as `"filesystem"`, `"git"`, or `"code_analysis"`.
    pub namespace: Option<String>,
    /// Semver-style version string for the tool (e.g. `"1.0.0"`).
    pub version: Option<String>,
}

impl ToolMetadata {
    /// Create metadata with a namespace.
    #[must_use]
    pub fn with_namespace(namespace: impl Into<String>) -> Self {
        Self {
            namespace: Some(namespace.into()),
            version: None,
        }
    }

    /// Set the version on this metadata.
    #[must_use]
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
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

    /// Optional organizational metadata (namespace, version).
    ///
    /// Returns `None` by default for backward compatibility.
    fn metadata(&self) -> Option<ToolMetadata> {
        None
    }

    /// Optional rich context for the approval UI.
    ///
    /// When a tool call requires approval, this method is called to provide
    /// additional context (e.g., a diff preview, estimated cost, query plan).
    /// The returned value is attached to the [`ToolApprovalRequest`].
    ///
    /// Returns `None` by default — tools work fine without it. Panics are
    /// caught and treated as `None`.
    fn approval_context(&self, _params: &Value) -> Option<Value> {
        None
    }

    /// Optional authentication configuration for this tool.
    ///
    /// When `Some`, the framework resolves credentials from the configured
    /// [`CredentialResolver`](crate::CredentialResolver) before calling
    /// [`execute()`](Self::execute). Returns `None` by default (no auth required).
    fn auth_config(&self) -> Option<crate::credential::AuthConfig> {
        None
    }

    /// Execute the tool with validated parameters.
    ///
    /// # Arguments
    ///
    /// * `tool_call_id` — unique identifier for this particular invocation
    /// * `params` — validated input parameters as a JSON value
    /// * `cancellation_token` — token that signals the tool should abort
    /// * `on_update` — optional callback for streaming partial results
    /// * `state` — shared session state for reading/writing structured data
    /// * `credential` — resolved credential if `auth_config()` returns `Some`
    fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        state: Arc<std::sync::RwLock<crate::SessionState>>,
        credential: Option<crate::credential::ResolvedCredential>,
    ) -> ToolFuture<'_>;
}

// ─── IntoTool ────────────────────────────────────────────────────────────────

/// Convenience trait to convert a tool implementation into `Arc<dyn AgentTool>`.
///
/// This eliminates the `Arc::new(tool) as Arc<dyn AgentTool>` ceremony.
///
/// # Example
///
/// ```ignore
/// use swink_agent::{IntoTool, BashTool};
/// let tools = vec![BashTool::new().into_tool()];
/// ```
pub trait IntoTool {
    /// Wrap this tool in an `Arc<dyn AgentTool>`.
    fn into_tool(self) -> Arc<dyn AgentTool>;
}

impl<T: AgentTool + 'static> IntoTool for T {
    fn into_tool(self) -> Arc<dyn AgentTool> {
        Arc::new(self)
    }
}

// ─── Validation ──────────────────────────────────────────────────────────────

/// Validate that a JSON value is a valid JSON Schema document.
///
/// This checks the schema itself for correctness (e.g., valid `type` values,
/// proper structure). Distinct from [`validate_tool_arguments`], which validates
/// data *against* a schema.
///
/// # Errors
///
/// Returns `Err(String)` when the schema document is invalid, with a
/// human-readable description of the problem.
pub fn validate_schema(schema: &Value) -> Result<(), String> {
    compiled_validator(schema)?;
    Ok(())
}

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
    let validator = compiled_validator(schema).map_err(|e| vec![e])?;

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

fn compiled_validator(schema: &Value) -> Result<Arc<jsonschema::Validator>, String> {
    let cache_key = serde_json::to_string(schema).map_err(|e| e.to_string())?;

    {
        let cache = SCHEMA_VALIDATOR_CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(validator) = cache.get(&cache_key) {
            return Ok(Arc::clone(validator));
        }
    }

    let compiled = Arc::new(jsonschema::validator_for(schema).map_err(|e| e.to_string())?);
    let mut cache = SCHEMA_VALIDATOR_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Ok(Arc::clone(
        cache.entry(cache_key).or_insert_with(|| Arc::clone(&compiled)),
    ))
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolApproval {
    /// The tool call is approved and should proceed.
    Approved,
    /// The tool call is rejected and should not execute.
    Rejected,
    /// Approved with modified parameters (constrain scope or sanitize input).
    ApprovedWith(serde_json::Value),
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
    /// Optional rich context from the tool's `approval_context()` method.
    pub context: Option<Value>,
}

impl fmt::Debug for ToolApprovalRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolApprovalRequest")
            .field("tool_call_id", &self.tool_call_id)
            .field("tool_name", &self.tool_name)
            .field("arguments", &"[REDACTED]")
            .field("requires_approval", &self.requires_approval)
            .field("context", &self.context)
            .finish()
    }
}

/// Controls whether the approval gate is active.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ApprovalMode {
    /// Every tool call goes through the approval callback.
    Enabled,
    /// Auto-approve read-only tools (where `requires_approval()` returns false);
    /// prompt for all others. Supports per-tool session trust.
    #[default]
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
) -> Box<ApproveToolFn>
where
    F: Fn(ToolApprovalRequest) -> ApproveToolFuture + Send + Sync + 'static,
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
        && SENSITIVE_KEYS.iter().any(|&s| key.eq_ignore_ascii_case(s))
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
        Value::Array(arr) => Value::Array(arr.iter().map(|v| redact_value(v, None)).collect()),
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

// ─── ToolParameters trait ───────────────────────────────────────────────────

/// Trait for types that can produce a JSON Schema describing their fields.
///
/// Implemented automatically by `#[derive(ToolSchema)]` from the
/// `swink-agent-macros` crate.
pub trait ToolParameters {
    /// Generate a JSON Schema for this type's fields.
    fn json_schema() -> Value;
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
            context: None,
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
        assert_eq!(redacted, json!(["normal", "[REDACTED]", "also normal"]));
    }

    #[test]
    fn handles_null_and_numbers() {
        let val = json!({"a": null, "b": 42, "c": 2.72});
        let redacted = redact_sensitive_values(&val);
        assert_eq!(redacted, val);
    }

    // ─── validate_schema ──────────────────────────────────────────────────

    #[test]
    fn valid_schema_passes() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "required": ["name"]
        });
        assert!(validate_schema(&schema).is_ok());
    }

    #[test]
    fn invalid_schema_returns_error() {
        let schema = json!({
            "type": "not_a_real_type"
        });
        assert!(validate_schema(&schema).is_err());
    }

    #[test]
    fn empty_object_schema_is_valid() {
        let schema = json!({
            "type": "object",
            "properties": {}
        });
        assert!(validate_schema(&schema).is_ok());
    }

    // ─── ApprovalMode ─────────────────────────────────────────────────────

    #[test]
    fn approval_mode_default_is_smart() {
        assert_eq!(ApprovalMode::default(), ApprovalMode::Smart);
    }

    #[test]
    fn approval_mode_variants_are_distinct() {
        assert_ne!(ApprovalMode::Enabled, ApprovalMode::Smart);
        assert_ne!(ApprovalMode::Smart, ApprovalMode::Bypassed);
        assert_ne!(ApprovalMode::Enabled, ApprovalMode::Bypassed);
    }

    // ─── ToolMetadata ────────────────────────────────────────────────────

    #[test]
    fn tool_metadata_default_is_empty() {
        let meta = ToolMetadata::default();
        assert_eq!(meta.namespace, None);
        assert_eq!(meta.version, None);
    }

    #[test]
    fn tool_metadata_builder() {
        let meta = ToolMetadata::with_namespace("filesystem").with_version("1.2.0");
        assert_eq!(meta.namespace.as_deref(), Some("filesystem"));
        assert_eq!(meta.version.as_deref(), Some("1.2.0"));
    }

    #[test]
    fn agent_tool_metadata_defaults_to_none() {
        use tokio_util::sync::CancellationToken;

        struct MinimalTool;

        impl AgentTool for MinimalTool {
            fn name(&self) -> &str {
                "minimal"
            }
            fn label(&self) -> &str {
                "Minimal"
            }
            fn description(&self) -> &str {
                "A minimal tool"
            }
            fn parameters_schema(&self) -> &Value {
                &Value::Null
            }
            fn execute(
                &self,
                _tool_call_id: &str,
                _params: Value,
                _ct: CancellationToken,
                _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
                _state: Arc<std::sync::RwLock<crate::SessionState>>,
                _credential: Option<crate::credential::ResolvedCredential>,
            ) -> ToolFuture<'_> {
                Box::pin(async { AgentToolResult::text("ok") })
            }
        }

        let tool = MinimalTool;
        assert!(tool.metadata().is_none());
    }

    // T025: auth_config default returns None
    #[test]
    fn agent_tool_auth_config_defaults_to_none() {
        use tokio_util::sync::CancellationToken;

        struct NoAuthTool;

        impl AgentTool for NoAuthTool {
            fn name(&self) -> &str { "no-auth" }
            fn label(&self) -> &str { "No Auth" }
            fn description(&self) -> &str { "Tool with no auth" }
            fn parameters_schema(&self) -> &Value { &Value::Null }
            fn execute(
                &self,
                _tool_call_id: &str,
                _params: Value,
                _ct: CancellationToken,
                _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
                _state: Arc<std::sync::RwLock<crate::SessionState>>,
                _credential: Option<crate::credential::ResolvedCredential>,
            ) -> ToolFuture<'_> {
                Box::pin(async { AgentToolResult::text("ok") })
            }
        }

        let tool = NoAuthTool;
        assert!(tool.auth_config().is_none());
    }

    // ─── approval_context ────────────────────────────────────────────────

    #[test]
    fn approval_context_default_none() {
        use tokio_util::sync::CancellationToken;

        struct PlainTool;

        impl AgentTool for PlainTool {
            fn name(&self) -> &str { "plain" }
            fn label(&self) -> &str { "Plain" }
            fn description(&self) -> &str { "No context" }
            fn parameters_schema(&self) -> &Value { &Value::Null }
            fn execute(
                &self,
                _tool_call_id: &str,
                _params: Value,
                _ct: CancellationToken,
                _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
                _state: Arc<std::sync::RwLock<crate::SessionState>>,
                _credential: Option<crate::credential::ResolvedCredential>,
            ) -> ToolFuture<'_> {
                Box::pin(async { AgentToolResult::text("ok") })
            }
        }

        let tool = PlainTool;
        assert!(tool.approval_context(&json!({})).is_none());
    }

    #[test]
    fn approval_context_returns_value() {
        use tokio_util::sync::CancellationToken;

        struct ContextTool;

        impl AgentTool for ContextTool {
            fn name(&self) -> &str { "ctx" }
            fn label(&self) -> &str { "Ctx" }
            fn description(&self) -> &str { "With context" }
            fn parameters_schema(&self) -> &Value { &Value::Null }
            fn approval_context(&self, params: &Value) -> Option<Value> {
                Some(json!({"preview": format!("Will process: {}", params)}))
            }
            fn execute(
                &self,
                _tool_call_id: &str,
                _params: Value,
                _ct: CancellationToken,
                _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
                _state: Arc<std::sync::RwLock<crate::SessionState>>,
                _credential: Option<crate::credential::ResolvedCredential>,
            ) -> ToolFuture<'_> {
                Box::pin(async { AgentToolResult::text("ok") })
            }
        }

        let tool = ContextTool;
        let ctx = tool.approval_context(&json!({"file": "test.txt"}));
        assert!(ctx.is_some());
        assert!(ctx.unwrap()["preview"].as_str().unwrap().contains("test.txt"));
    }

    #[test]
    fn approval_context_panic_caught() {
        use tokio_util::sync::CancellationToken;

        struct PanickingTool;

        impl AgentTool for PanickingTool {
            fn name(&self) -> &str { "panicker" }
            fn label(&self) -> &str { "Panicker" }
            fn description(&self) -> &str { "Panics in context" }
            fn parameters_schema(&self) -> &Value { &Value::Null }
            fn approval_context(&self, _params: &Value) -> Option<Value> {
                panic!("oops");
            }
            fn execute(
                &self,
                _tool_call_id: &str,
                _params: Value,
                _ct: CancellationToken,
                _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
                _state: Arc<std::sync::RwLock<crate::SessionState>>,
                _credential: Option<crate::credential::ResolvedCredential>,
            ) -> ToolFuture<'_> {
                Box::pin(async { AgentToolResult::text("ok") })
            }
        }

        let tool = PanickingTool;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            tool.approval_context(&json!({}))
        }));
        // Panic is caught
        assert!(result.is_err());
    }

    #[test]
    fn approval_request_includes_context() {
        let ctx = json!({"diff": "+new line"});
        let req = ToolApprovalRequest {
            tool_call_id: "call_1".into(),
            tool_name: "write_file".into(),
            arguments: json!({"path": "/tmp/test"}),
            requires_approval: true,
            context: Some(ctx.clone()),
        };
        assert_eq!(req.context, Some(ctx));
    }
}
