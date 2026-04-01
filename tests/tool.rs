mod common;

use common::MockTool;
use serde_json::{Value, json};
use std::sync::Arc;
use swink_agent::{
    AgentTool, AgentToolResult, ContentBlock, unknown_tool_result, validate_tool_arguments,
    validation_error_result,
};
use tokio_util::sync::CancellationToken;

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

#[test]
fn mock_tool_schema_validates_good_args() {
    let tool = MockTool::new("mock_tool").with_schema(sample_schema());
    let args = json!({"path": "/etc/hosts"});
    assert!(validate_tool_arguments(tool.parameters_schema(), &args).is_ok());
}

#[test]
fn mock_tool_schema_rejects_bad_args() {
    let tool = MockTool::new("mock_tool").with_schema(sample_schema());
    let args = json!({"wrong_field": 42});
    assert!(validate_tool_arguments(tool.parameters_schema(), &args).is_err());
}

#[test]
fn mock_tool_is_object_safe() {
    let tool: Arc<dyn AgentTool> = Arc::new(MockTool::new("mock_tool"));
    assert_eq!(tool.name(), "mock_tool");
    assert_eq!(tool.label(), "mock_tool");
    assert!(!tool.description().is_empty());
}

#[tokio::test]
async fn mock_tool_executes() {
    let tool = MockTool::new("mock_tool")
        .with_schema(sample_schema())
        .with_result(AgentToolResult::text("read file: /tmp/x"));
    let token = CancellationToken::new();
    let result = tool
        .execute("tc_1", json!({"path": "/tmp/x"}), token, None, std::sync::Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())), None)
        .await;
    assert_eq!(result.content.len(), 1);
    assert!(matches!(&result.content[0], ContentBlock::Text { text } if text.contains("/tmp/x")));
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

// ── T017: Empty parameter schema accepts empty args ──

#[test]
fn empty_schema_accepts_empty_args() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {}
    });
    let args = serde_json::json!({});
    assert!(validate_tool_arguments(&schema, &args).is_ok());
}

// ── T018: is_error flag distinction ──

#[test]
fn is_error_flag_distinction() {
    let success = AgentToolResult::text("ok");
    assert!(!success.is_error);

    let failure = AgentToolResult::error("bad");
    assert!(failure.is_error);
}
