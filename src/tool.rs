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
#[derive(Debug, Clone)]
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

/// Controls whether the approval gate is active.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ApprovalMode {
    /// Every tool call goes through the approval callback.
    #[default]
    Enabled,
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

// ─── Compile-time Send + Sync assertions ────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AgentToolResult>();
    assert_send_sync::<ToolApproval>();
    assert_send_sync::<ToolApprovalRequest>();
    assert_send_sync::<ApprovalMode>();
};
