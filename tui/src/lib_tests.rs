//! Tests for `lib`.
#![cfg(test)]

use super::*;
use tokio::sync::mpsc;

fn launcher_options() -> swink_agent::AgentOptions {
    use swink_agent::testing::SimpleMockStreamFn;
    swink_agent::AgentOptions::new_simple(
        "system",
        swink_agent::ModelSpec::new("mock", "test"),
        std::sync::Arc::new(SimpleMockStreamFn::from_text("hi")),
    )
}

#[tokio::test]
async fn launcher_build_assembles_agent_extensions_and_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();
    let extensions = TuiExtensions::new().with_command("marker", |_app, _args| {
        CustomCommandOutcome::Feedback("ok".into())
    });

    let app = TuiLauncher::new(TuiConfig::default())
        .with_extensions(extensions)
        .with_session_store(store, "launcher-session".to_string())
        .build(launcher_options())
        .unwrap();

    assert!(app.agent_io.agent.is_some(), "agent must be constructed");
    assert_eq!(app.session.session_id, "launcher-session");
    assert!(
        app.extensions.command_names().any(|name| name == "marker"),
        "host command must be registered"
    );
}

#[tokio::test]
async fn launcher_resume_of_missing_session_fails() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let result = TuiLauncher::new(TuiConfig::default())
        .with_session_store(store, "fresh".to_string())
        .with_resume("does-not-exist".to_string())
        .build(launcher_options());
    assert!(result.is_err(), "resuming a nonexistent session must fail");
}

fn approval_request() -> ToolApprovalRequest {
    ToolApprovalRequest::new(
        "call_1",
        "write_file",
        serde_json::json!({"path": "secret.txt"}),
        true,
    )
}

fn read_only_approval_request() -> ToolApprovalRequest {
    ToolApprovalRequest::new(
        "call_read",
        "read_file",
        serde_json::json!({"path": "notes.md"}),
        false,
    )
}

#[test]
fn explicit_param_wins_over_config() {
    let config = TuiConfig {
        system_prompt: Some("from config".to_string()),
        ..TuiConfig::default()
    };
    let result = resolve_system_prompt(Some("explicit".to_string()), &config);
    assert_eq!(result, "explicit");
}

#[test]
fn explicit_param_wins_with_no_config() {
    let config = TuiConfig::default();
    let result = resolve_system_prompt(Some("explicit".to_string()), &config);
    assert_eq!(result, "explicit");
}

#[test]
fn config_used_when_no_explicit_param() {
    // This test is valid regardless of env var state because explicit=Some always
    // wins. When explicit=None and env var is unset, config should win.
    // If LLM_SYSTEM_PROMPT happens to be set in the environment, the env var
    // will win over config -- that is the correct priority order.
    let config = TuiConfig {
        system_prompt: Some("from config".to_string()),
        ..TuiConfig::default()
    };
    let result = resolve_system_prompt(None, &config);
    // Result is either "from config" (no env var) or env var value (env var set).
    // We verify it is NOT the default, which proves the fallback chain works.
    assert_ne!(result, DEFAULT_SYSTEM_PROMPT);
}

#[test]
fn default_fallback_when_nothing_set() {
    // When no explicit param AND no config system_prompt AND no LLM_SYSTEM_PROMPT
    // env var, should return the default constant.
    // Note: if LLM_SYSTEM_PROMPT is set in the test environment, this test
    // verifies that the env var path is taken instead (which is correct behavior).
    let config = TuiConfig::default();
    assert!(config.system_prompt.is_none());
    let result = resolve_system_prompt(None, &config);
    // Either the default constant or the env var -- both are valid outcomes
    if std::env::var("LLM_SYSTEM_PROMPT").is_err() {
        assert_eq!(result, DEFAULT_SYSTEM_PROMPT);
    }
}

#[test]
fn explicit_empty_string_still_wins() {
    let config = TuiConfig {
        system_prompt: Some("from config".to_string()),
        ..TuiConfig::default()
    };
    let result = resolve_system_prompt(Some(String::new()), &config);
    assert_eq!(result, "", "explicit empty string should still be used");
}

#[test]
fn default_system_prompt_is_not_empty() {
    assert!(!DEFAULT_SYSTEM_PROMPT.is_empty());
}

#[tokio::test]
async fn approval_callback_rejects_when_channel_send_fails() {
    let (tx, rx) = mpsc::channel(1);
    drop(rx);

    let callback = tui_approval_callback(&tx);
    let approval = callback(approval_request()).await;

    assert_eq!(approval, ToolApproval::Rejected);
}

#[tokio::test]
async fn approval_callback_rejects_when_responder_drops() {
    let (tx, mut rx) = mpsc::channel(1);
    let callback = tui_approval_callback(&tx);

    let approval_task = tokio::spawn(async move { callback(approval_request()).await });

    let (_, responder) = rx
        .recv()
        .await
        .expect("approval request should be forwarded");
    drop(responder);

    assert_eq!(approval_task.await.unwrap(), ToolApproval::Rejected);
}

#[tokio::test]
async fn approval_callback_forwards_read_only_requests_to_tui() {
    let (tx, mut rx) = mpsc::channel(1);
    let callback = tui_approval_callback(&tx);

    let approval_task = tokio::spawn(async move { callback(read_only_approval_request()).await });

    for _ in 0..10 {
        tokio::task::yield_now().await;
    }

    let (request, responder) = rx
        .try_recv()
        .expect("read-only approval request should be forwarded");
    assert_eq!(request.tool_name, "read_file");
    assert!(!request.requires_approval);

    responder.send(ToolApproval::Approved).unwrap();
    assert_eq!(approval_task.await.unwrap(), ToolApproval::Approved);
}
