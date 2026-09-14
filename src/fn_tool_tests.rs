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

fn bearer_auth() -> crate::AuthConfig {
    crate::AuthConfig::new(
        "api",
        crate::AuthScheme::BearerHeader,
        crate::CredentialType::Bearer,
    )
}

#[tokio::test]
async fn context_execute_receives_session_state() {
    let tool = FnTool::new("ctx", "Ctx", "Context signature.").with_execute_context(
        |id, _params, _cancel, _on_update, state, credential| async move {
            let seen: String = state.read().unwrap().get("seed").unwrap();
            state.write().unwrap().set("written", &id).unwrap();
            AgentToolResult::text(format!("{seen}:{}", credential.is_none()))
        },
    );
    let state = test_state();
    state.write().unwrap().set("seed", "hello").unwrap();

    let result = tool
        .execute(
            "call_7",
            json!({}),
            CancellationToken::new(),
            None,
            std::sync::Arc::clone(&state),
            None,
        )
        .await;

    assert_eq!(ContentBlock::extract_text(&result.content), "hello:true");
    let written: String = state.read().unwrap().get("written").unwrap();
    assert_eq!(written, "call_7");
}

#[tokio::test]
async fn context_execute_receives_resolved_credential() {
    let tool = FnTool::new("auth", "Auth", "Authenticated.")
        .with_auth_config(bearer_auth())
        .with_execute_context(
            |_id, _params, _cancel, _on_update, _state, credential| async move {
                match credential {
                    Some(crate::ResolvedCredential::Bearer(token)) => AgentToolResult::text(token),
                    _ => AgentToolResult::error("no bearer"),
                }
            },
        );

    let result = tool
        .execute(
            "id",
            json!({}),
            CancellationToken::new(),
            None,
            test_state(),
            Some(crate::ResolvedCredential::Bearer("tok-123".into())),
        )
        .await;

    assert!(!result.is_error);
    assert_eq!(ContentBlock::extract_text(&result.content), "tok-123");
}

#[test]
fn auth_config_returns_configured_value() {
    assert!(sample_tool().auth_config().is_none());

    let tool = sample_tool().with_auth_config(crate::AuthConfig::new(
        "weather-key",
        crate::AuthScheme::ApiKeyHeader("X-Api-Key".into()),
        crate::CredentialType::ApiKey,
    ));
    let config = tool.auth_config().expect("auth config set");
    assert_eq!(config.credential_key, "weather-key");
    assert!(
        matches!(config.auth_scheme, crate::AuthScheme::ApiKeyHeader(ref h) if h == "X-Api-Key")
    );
    assert!(matches!(
        config.credential_type,
        crate::CredentialType::ApiKey
    ));
}

#[tokio::test]
async fn lightweight_constructors_unaffected_by_state_and_credential() {
    let tool = FnTool::new("simple", "Simple", "Simple.")
        .with_auth_config(bearer_auth())
        .with_execute_simple(|params, _cancel| async move {
            AgentToolResult::text(params["msg"].as_str().unwrap_or("none").to_owned())
        });

    let result = tool
        .execute(
            "id",
            json!({"msg": "unchanged"}),
            CancellationToken::new(),
            None,
            test_state(),
            Some(crate::ResolvedCredential::Bearer("tok".into())),
        )
        .await;

    assert!(!result.is_error);
    assert_eq!(ContentBlock::extract_text(&result.content), "unchanged");
}
