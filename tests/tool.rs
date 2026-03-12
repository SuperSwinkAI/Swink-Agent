use swink_agent::{
    AgentTool, AgentToolResult, ContentBlock, unknown_tool_result, validate_tool_arguments,
    validation_error_result,
};
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
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
