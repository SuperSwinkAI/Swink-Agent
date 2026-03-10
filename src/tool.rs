//! Tool system traits and validation for the agent harness.
//!
//! This module defines the [`AgentTool`] trait that all tools must implement,
//! the [`AgentToolResult`] type returned by tool execution, and the
//! [`validate_tool_arguments`] function for validating tool call arguments
//! against a JSON Schema.

use std::future::Future;
use std::pin::Pin;

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
pub fn unknown_tool_result(tool_name: &str) -> AgentToolResult {
    AgentToolResult::error(format!("unknown tool: {tool_name}"))
}

/// Build an error result listing all validation errors.
pub fn validation_error_result(errors: &[String]) -> AgentToolResult {
    let message = errors.join("\n");
    AgentToolResult::error(message)
}

// ─── Compile-time Send + Sync assertions ────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AgentToolResult>();
};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    // ── Helper: sample JSON Schema ──

    fn sample_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "line": { "type": "integer" }
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    // ── 2.1: Valid arguments pass validation ──

    #[test]
    fn valid_arguments_pass_validation() {
        let schema = sample_schema();
        let args = json!({"path": "/tmp/file.txt", "line": 42});
        assert!(validate_tool_arguments(&schema, &args).is_ok());
    }

    #[test]
    fn valid_arguments_minimal() {
        let schema = sample_schema();
        let args = json!({"path": "/tmp/file.txt"});
        assert!(validate_tool_arguments(&schema, &args).is_ok());
    }

    // ── 2.2: Invalid arguments produce field-level errors ──

    #[test]
    fn invalid_type_produces_errors() {
        let schema = sample_schema();
        let args = json!({"path": 123});
        let result = validate_tool_arguments(&schema, &args);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(!errors.is_empty());
        // The error should mention the type mismatch.
        let combined = errors.join(" ");
        assert!(
            combined.contains("123") || combined.contains("type") || combined.contains("string"),
            "expected field-level error mentioning the type issue, got: {combined}"
        );
    }

    #[test]
    fn invalid_integer_field_produces_errors() {
        let schema = sample_schema();
        let args = json!({"path": "/tmp", "line": "not a number"});
        let result = validate_tool_arguments(&schema, &args);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(!errors.is_empty());
    }

    // ── 2.3: Missing required fields are caught ──

    #[test]
    fn missing_required_field_caught() {
        let schema = sample_schema();
        let args = json!({"line": 10});
        let result = validate_tool_arguments(&schema, &args);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        let combined = errors.join(" ");
        assert!(
            combined.contains("path") || combined.contains("required"),
            "expected error about missing 'path', got: {combined}"
        );
    }

    #[test]
    fn empty_object_missing_required_field() {
        let schema = sample_schema();
        let args = json!({});
        let result = validate_tool_arguments(&schema, &args);
        assert!(result.is_err());
    }

    // ── 2.4: Extra fields with additionalProperties=false are caught ──

    #[test]
    fn extra_fields_rejected() {
        let schema = sample_schema();
        let args = json!({"path": "/tmp", "extra_field": true});
        let result = validate_tool_arguments(&schema, &args);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        let combined = errors.join(" ");
        assert!(
            combined.contains("extra_field") || combined.contains("additional"),
            "expected error about extra field, got: {combined}"
        );
    }

    // ── 2.11: A mock AgentTool can be constructed and its schema validated ──

    struct MockTool {
        schema: Value,
    }

    impl MockTool {
        fn new() -> Self {
            Self {
                schema: sample_schema(),
            }
        }
    }

    #[allow(clippy::unnecessary_literal_bound)]
    impl AgentTool for MockTool {
        fn name(&self) -> &str {
            "mock_tool"
        }

        fn label(&self) -> &str {
            "Mock Tool"
        }

        fn description(&self) -> &str {
            "A mock tool for testing purposes."
        }

        fn parameters_schema(&self) -> &Value {
            &self.schema
        }

        fn execute(
            &self,
            _tool_call_id: &str,
            params: Value,
            _cancellation_token: CancellationToken,
            _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
            Box::pin(async move {
                let path = params
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                AgentToolResult::text(format!("read file: {path}"))
            })
        }
    }

    #[test]
    fn mock_tool_schema_validates_good_args() {
        let tool = MockTool::new();
        let args = json!({"path": "/etc/hosts"});
        assert!(validate_tool_arguments(tool.parameters_schema(), &args).is_ok());
    }

    #[test]
    fn mock_tool_schema_rejects_bad_args() {
        let tool = MockTool::new();
        let args = json!({"wrong_field": 42});
        assert!(validate_tool_arguments(tool.parameters_schema(), &args).is_err());
    }

    #[test]
    fn mock_tool_is_object_safe() {
        let tool: Arc<dyn AgentTool> = Arc::new(MockTool::new());
        assert_eq!(tool.name(), "mock_tool");
        assert_eq!(tool.label(), "Mock Tool");
        assert!(!tool.description().is_empty());
    }

    #[tokio::test]
    async fn mock_tool_executes() {
        let tool = MockTool::new();
        let token = CancellationToken::new();
        let result = tool
            .execute("tc_1", json!({"path": "/tmp/x"}), token, None)
            .await;
        assert_eq!(result.content.len(), 1);
        assert!(
            matches!(&result.content[0], ContentBlock::Text { text } if text.contains("/tmp/x"))
        );
    }

    // ── Helper constructors ──

    #[test]
    fn text_result_constructor() {
        let result = AgentToolResult::text("hello");
        assert_eq!(result.content.len(), 1);
        assert!(matches!(&result.content[0], ContentBlock::Text { text } if text == "hello"));
        assert_eq!(result.details, Value::Null);
    }

    #[test]
    fn error_result_constructor() {
        let result = AgentToolResult::error("something went wrong");
        assert_eq!(result.content.len(), 1);
        assert!(
            matches!(&result.content[0], ContentBlock::Text { text } if text == "something went wrong")
        );
        assert_eq!(result.details, Value::Null);
    }

    #[test]
    fn unknown_tool_result_message() {
        let result = unknown_tool_result("nonexistent");
        assert!(
            matches!(&result.content[0], ContentBlock::Text { text } if text == "unknown tool: nonexistent")
        );
    }

    #[test]
    fn validation_error_result_message() {
        let errors = vec![
            "missing field: path".to_string(),
            "invalid type for line".to_string(),
        ];
        let result = validation_error_result(&errors);
        assert!(
            matches!(&result.content[0], ContentBlock::Text { text } if text.contains("missing field: path") && text.contains("invalid type for line"))
        );
    }

    // ── Send + Sync ──

    #[test]
    fn agent_tool_result_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AgentToolResult>();
    }

    #[test]
    fn dyn_agent_tool_is_send_sync() {
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<dyn AgentTool>();
    }
}
