//! Tests for `fn_tool`.
#![cfg(test)]

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::ContentBlock;

fn test_state() -> std::sync::Arc<std::sync::RwLock<crate::SessionState>> {
    std::sync::Arc::new(std::sync::RwLock::new(crate::SessionState::new()))
}

fn sample_tool() -> FnTool {
    FnTool::new("test", "Test", "A test tool.")
}

#[test]
fn metadata_matches_constructor() {
    let tool = sample_tool();
    assert_eq!(tool.name(), "test");
    assert_eq!(tool.label(), "Test");
    assert_eq!(tool.description(), "A test tool.");
    assert!(!tool.requires_approval());
}

#[tokio::test]
async fn default_execute_returns_error() {
    let tool = sample_tool();
    let result = tool
        .execute(
            "{}",
            json!({}),
            CancellationToken::new(),
            None,
            test_state(),
            None,
        )
        .await;
    assert!(result.is_error);
}

#[tokio::test]
async fn simple_execute_receives_params() {
    let tool = FnTool::new("echo", "Echo", "Echo params.").with_execute_simple(
        |params, _cancel| async move {
            let msg = params["msg"].as_str().unwrap_or("none").to_owned();
            AgentToolResult::text(msg)
        },
    );

    let result = tool
        .execute(
            "id",
            json!({"msg": "hello"}),
            CancellationToken::new(),
            None,
            test_state(),
            None,
        )
        .await;
    assert!(!result.is_error);
    assert_eq!(result.content.len(), 1);
}

#[tokio::test]
async fn async_execute_receives_params() {
    let tool = FnTool::new("echo", "Echo", "Echo params.").with_execute_async(
        |params, _cancel| async move {
            let msg = params["msg"].as_str().unwrap_or("none").to_owned();
            AgentToolResult::text(msg)
        },
    );

    let result = tool
        .execute(
            "id",
            json!({"msg": "hello"}),
            CancellationToken::new(),
            None,
            test_state(),
            None,
        )
        .await;
    assert!(!result.is_error);
    assert_eq!(ContentBlock::extract_text(&result.content), "hello");
}

#[derive(Deserialize, JsonSchema)]
#[allow(dead_code)]
struct TestParams {
    city: String,
}

#[test]
fn with_schema_for_sets_schema() {
    let tool = sample_tool().with_schema_for::<TestParams>();
    let schema = tool.parameters_schema();
    assert_eq!(schema["type"], "object");
    assert!(
        schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("city"))
    );
}

#[test]
fn approval_flag_is_configurable() {
    let tool = sample_tool().with_requires_approval(true);
    assert!(tool.requires_approval());
}

#[test]
fn execution_root_is_configurable() {
    let root = std::path::PathBuf::from("workspace");
    let tool = sample_tool().with_execution_root(&root);
    assert_eq!(tool.execution_root(), Some(root.as_path()));
}

#[tokio::test]
async fn full_execute_receives_all_args() {
    let tool = FnTool::new("full", "Full", "Full signature.").with_execute(
        |id, _params, _cancel, _on_update| async move { AgentToolResult::text(format!("id={id}")) },
    );

    let result = tool
        .execute(
            "call_42",
            json!({}),
            CancellationToken::new(),
            None,
            test_state(),
            None,
        )
        .await;
    assert!(!result.is_error);
}

#[derive(Deserialize, JsonSchema)]
struct TypedParams {
    city: String,
}

#[tokio::test]
async fn typed_execute_deserializes_params_and_sets_schema() {
    let tool = FnTool::new("typed", "Typed", "Typed params.").with_execute_typed(
        |params: TypedParams, _cancel| async move { AgentToolResult::text(params.city) },
    );

    let schema = tool.parameters_schema();
    assert_eq!(schema["type"], "object");
    assert!(
        schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("city"))
    );

    let result = tool
        .execute(
            "id",
            json!({"city": "Chicago"}),
            CancellationToken::new(),
            None,
            test_state(),
            None,
        )
        .await;
    assert!(!result.is_error);
    assert_eq!(ContentBlock::extract_text(&result.content), "Chicago");
}

#[tokio::test]
async fn typed_execute_reports_deserialization_errors() {
    let tool = FnTool::new("typed", "Typed", "Typed params.").with_execute_typed(
        |params: TypedParams, _cancel| async move { AgentToolResult::text(params.city) },
    );

    let result = tool
        .execute(
            "id",
            json!({"city": 42}),
            CancellationToken::new(),
            None,
            test_state(),
            None,
        )
        .await;
    assert!(result.is_error);
    assert!(
        ContentBlock::extract_text(&result.content).contains("invalid parameters"),
        "expected invalid parameters error, got: {:?}",
        result.content
    );
}
