//! Regression tests for the `#[tool]` attribute macro.
//!
//! Verifies that schema generation delegates to schemars (no bespoke type
//! mapper) and that the tool can be constructed and introspected.

#![allow(dead_code)]

use swink_agent::AgentTool;
use swink_agent_macros::tool;

// ── schema from a simple two-param tool ─────────────────────────────────────

#[tool(name = "greet", description = "Greet someone by name")]
async fn greet(name: String, times: u32) -> swink_agent::AgentToolResult {
    swink_agent::AgentToolResult::text(format!("{name} x{times}"))
}

#[test]
fn tool_attr_schema_basic() {
    let t = GreetTool;
    assert_eq!(t.name(), "greet");
    assert_eq!(t.description(), "Greet someone by name");

    let schema = t.parameters_schema();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["name"]["type"], "string");
    assert_eq!(schema["properties"]["times"]["type"], "integer");

    let required = schema["required"].as_array().unwrap();
    assert!(required.contains(&serde_json::json!("name")));
    assert!(required.contains(&serde_json::json!("times")));
}

// ── Option params are not required ──────────────────────────────────────────

#[tool(name = "search", description = "Run a search")]
async fn search(query: String, limit: Option<u32>) -> swink_agent::AgentToolResult {
    let _ = limit;
    swink_agent::AgentToolResult::text(query)
}

#[test]
fn tool_attr_schema_optional_param_not_required() {
    let schema = SearchTool.parameters_schema();
    let required = schema["required"].as_array().unwrap();
    assert!(required.contains(&serde_json::json!("query")));
    assert!(!required.contains(&serde_json::json!("limit")));
    // The optional field still appears in properties.
    assert!(schema["properties"]["limit"].is_object());
}

// ── zero-param tool ──────────────────────────────────────────────────────────

#[tool(name = "ping", description = "Ping with no params")]
async fn ping() -> swink_agent::AgentToolResult {
    swink_agent::AgentToolResult::text("pong")
}

#[test]
fn tool_attr_schema_no_params() {
    let schema = PingTool.parameters_schema();
    assert_eq!(schema["type"], "object");
    // No required fields.
    assert!(schema["required"].as_array().map_or(true, |a| a.is_empty()));
}

// ── CancellationToken not leaked into schema ─────────────────────────────────
//
// A CancellationToken parameter must be excluded from the generated schema and
// must not appear in `required`. This also verifies that the macro compiles
// correctly when CancellationToken is present (previously a bug: the token was
// excluded from the call args, causing an arity mismatch).

#[tool(
    name = "cancel_aware",
    description = "Tool that receives a cancellation token"
)]
async fn cancel_aware(
    message: String,
    cancel: tokio_util::sync::CancellationToken,
) -> swink_agent::AgentToolResult {
    let _ = cancel;
    swink_agent::AgentToolResult::text(message)
}

#[test]
fn tool_attr_cancellation_token_excluded_from_schema() {
    let schema = CancelAwareTool.parameters_schema();
    // CancellationToken must not appear in properties or required.
    assert!(schema["properties"].get("cancel").is_none());
    let required = schema["required"].as_array().unwrap();
    assert!(!required.contains(&serde_json::json!("cancel")));
    // The regular param is still present.
    assert_eq!(schema["properties"]["message"]["type"], "string");
    assert!(required.contains(&serde_json::json!("message")));
}
