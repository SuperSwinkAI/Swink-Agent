//! Tests for the `schema_for` helper and its integration with built-in tools.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use swink_agent::{
    BashTool, ReadFileTool, WriteFileTool, schema_for, validate_schema, validate_tool_arguments,
};

// ─── schema_for generates valid schemas ─────────────────────────────────────

#[derive(Deserialize, JsonSchema)]
#[allow(dead_code)]
struct Simple {
    /// A name field.
    name: String,
    /// An optional count.
    count: Option<u32>,
}

#[test]
fn schema_for_generates_valid_schema() {
    let schema = schema_for::<Simple>();
    assert!(validate_schema(&schema).is_ok());
    assert_eq!(schema["type"], "object");
}

#[test]
fn schema_for_includes_required_fields() {
    let schema = schema_for::<Simple>();
    let required = schema["required"].as_array().expect("required array");
    assert!(required.contains(&json!("name")));
}

#[test]
fn schema_for_optional_fields_not_required() {
    let schema = schema_for::<Simple>();
    let required = schema["required"].as_array().expect("required array");
    assert!(!required.contains(&json!("count")));
}

// ─── Built-in tool schemas still validate correctly ─────────────────────────

#[test]
fn bash_tool_schema_validates_valid_args() {
    use swink_agent::AgentTool;
    let tool = BashTool::new();
    let args = json!({"command": "echo hello"});
    assert!(validate_tool_arguments(tool.parameters_schema(), &args).is_ok());
}

#[test]
fn bash_tool_schema_rejects_missing_command() {
    use swink_agent::AgentTool;
    let tool = BashTool::new();
    let args = json!({"timeout_ms": 5000});
    assert!(validate_tool_arguments(tool.parameters_schema(), &args).is_err());
}

#[test]
fn read_file_tool_schema_validates_valid_args() {
    use swink_agent::AgentTool;
    let tool = ReadFileTool::new();
    let args = json!({"path": "/tmp/file.txt"});
    assert!(validate_tool_arguments(tool.parameters_schema(), &args).is_ok());
}

#[test]
fn write_file_tool_schema_validates_valid_args() {
    use swink_agent::AgentTool;
    let tool = WriteFileTool::new();
    let args = json!({"path": "/tmp/file.txt", "content": "hello"});
    assert!(validate_tool_arguments(tool.parameters_schema(), &args).is_ok());
}

#[test]
fn write_file_tool_schema_rejects_extra_fields() {
    use swink_agent::AgentTool;
    let tool = WriteFileTool::new();
    let args = json!({"path": "/tmp/file.txt", "content": "hello", "extra": true});
    assert!(validate_tool_arguments(tool.parameters_schema(), &args).is_err());
}
