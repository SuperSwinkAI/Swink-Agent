use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use swink_agent::testing::ScriptedStreamFn;
use swink_agent::{AgentTool, ApprovalMode, ToolApproval, ToolApprovalRequest};

use crate::config::TuiConfig;

use super::super::state::TrustFollowUp;
use super::super::*;
use super::helpers::*;

/// Build a minimal `App` with the given `ApprovalMode` installed on the agent.
fn make_app_with_mode(mode: ApprovalMode) -> App {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let mut agent = make_test_agent(stream_fn);
    agent.set_approval_mode(mode);
    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);
    app
}

#[tokio::test]
async fn smart_mode_auto_approves_trusted_tool() {
    let mut app = make_app_with_mode(ApprovalMode::Smart);
    app.session_trusted_tools.insert("bash".to_string());

    let (tx, rx) = tokio::sync::oneshot::channel();
    let request = ToolApprovalRequest {
        tool_call_id: "call_1".into(),
        tool_name: "bash".into(),
        arguments: serde_json::json!({"command": "ls"}),
        requires_approval: true,
        context: None,
    };

    app.approval_tx.send((request, tx)).await.unwrap();

    let (req, responder) = app.approval_rx.recv().await.unwrap();
    app.handle_approval_request(req, responder);

    assert!(app.pending_approval.is_none());
    assert_eq!(rx.await.unwrap(), ToolApproval::Approved);
}

#[tokio::test]
async fn smart_mode_prompts_for_untrusted_tool() {
    let mut app = make_app_with_mode(ApprovalMode::Smart);

    let (tx, _rx) = tokio::sync::oneshot::channel();
    let request = ToolApprovalRequest {
        tool_call_id: "call_2".into(),
        tool_name: "write_file".into(),
        arguments: serde_json::json!({}),
        requires_approval: true,
        context: None,
    };

    app.handle_approval_request(request, tx);

    assert!(app.pending_approval.is_some());
}

#[tokio::test]
async fn always_approve_adds_to_trusted_set() {
    let mut app = App::new(TuiConfig::default());

    let (tx, rx) = tokio::sync::oneshot::channel();
    let request = ToolApprovalRequest {
        tool_call_id: "call_3".into(),
        tool_name: "bash".into(),
        arguments: serde_json::json!({}),
        requires_approval: true,
        context: None,
    };
    app.pending_approval = Some((request, tx));

    let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
    app.handle_key_event(key);

    assert!(app.session_trusted_tools.contains("bash"));
    assert!(app.pending_approval.is_none());
    assert_eq!(rx.await.unwrap(), ToolApproval::Approved);
}

#[tokio::test]
async fn reset_clears_trusted_tools() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent(stream_fn);

    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);
    app.session_trusted_tools.insert("bash".to_string());
    app.session_trusted_tools.insert("read_file".to_string());
    assert_eq!(app.session_trusted_tools.len(), 2);

    if let Some(agent) = &mut app.agent {
        agent.reset();
    }
    app.messages.clear();
    app.session_trusted_tools.clear();

    assert!(app.session_trusted_tools.is_empty());
}

#[tokio::test]
async fn query_approval_mode_shows_smart() {
    let mut app = make_app_with_mode(ApprovalMode::Smart);
    app.session_trusted_tools.insert("bash".to_string());

    let label = match app.approval_mode() {
        ApprovalMode::Enabled => "enabled",
        ApprovalMode::Bypassed => "disabled (auto-approve)",
        ApprovalMode::Smart => {
            "smart (auto-approve read-only and trusted tools, prompt for writes)"
        }
        _ => "unknown",
    };
    let mut msg = format!("Tool approval: {label}");
    if app.approval_mode() == ApprovalMode::Smart && !app.session_trusted_tools.is_empty() {
        msg.push_str("\nTrusted tools: ");
        let mut tools: Vec<&str> = app
            .session_trusted_tools
            .iter()
            .map(String::as_str)
            .collect();
        tools.sort_unstable();
        msg.push_str(&tools.join(", "));
    }

    assert!(msg.contains("smart"));
    assert!(msg.contains("Trusted tools: bash"));
}

// ─── Approval Modes ──────────────────────────────────────────

#[test]
fn approval_mode_default_is_smart() {
    let app = App::new(TuiConfig::default());
    assert_eq!(app.approval_mode(), ApprovalMode::Smart);
}

#[tokio::test]
async fn smart_mode_auto_approves_untrusted_readonly_tool() {
    let mut app = make_app_with_mode(ApprovalMode::Smart);

    let (tx, rx) = tokio::sync::oneshot::channel();
    let request = ToolApprovalRequest {
        tool_call_id: "call_ro".into(),
        tool_name: "read_file".into(),
        arguments: serde_json::json!({}),
        requires_approval: false,
        context: None,
    };

    app.handle_approval_request(request, tx);

    assert_eq!(rx.await.unwrap(), ToolApproval::Approved);
    assert!(app.pending_approval.is_none());
}

#[tokio::test]
async fn smart_mode_prompts_for_write_tool() {
    let mut app = make_app_with_mode(ApprovalMode::Smart);

    let (tx, _rx) = tokio::sync::oneshot::channel();
    let request = ToolApprovalRequest {
        tool_call_id: "call_w".into(),
        tool_name: "write_file".into(),
        arguments: serde_json::json!({}),
        requires_approval: true,
        context: None,
    };

    app.handle_approval_request(request, tx);
    assert!(app.pending_approval.is_some());
}

#[tokio::test]
async fn enabled_mode_prompts_for_all_tools() {
    let mut app = make_app_with_mode(ApprovalMode::Enabled);

    let (tx, _rx) = tokio::sync::oneshot::channel();
    let request = ToolApprovalRequest {
        tool_call_id: "call_r".into(),
        tool_name: "read_file".into(),
        arguments: serde_json::json!({}),
        requires_approval: false,
        context: None,
    };

    app.handle_approval_request(request, tx);
    assert!(
        app.pending_approval.is_some(),
        "Enabled mode should prompt for all tools"
    );
}

#[tokio::test]
async fn bypassed_mode_auto_approves_all() {
    let app = make_app_with_mode(ApprovalMode::Bypassed);
    assert_eq!(app.approval_mode(), ApprovalMode::Bypassed);
}

#[test]
fn approve_command_switches_modes() {
    use crate::commands::{ApprovalModeArg, CommandResult, execute_command};
    assert!(matches!(
        execute_command("#approve on"),
        CommandResult::SetApprovalMode(ApprovalModeArg::On)
    ));
    assert!(matches!(
        execute_command("#approve smart"),
        CommandResult::SetApprovalMode(ApprovalModeArg::Smart)
    ));
    assert!(matches!(
        execute_command("#approve off"),
        CommandResult::SetApprovalMode(ApprovalModeArg::Off)
    ));
}

// ─── Session Trust Follow-Up ──────────────────────────────────

#[tokio::test]
async fn trust_follow_up_triggers_after_approval_in_smart_mode() {
    let mut app = make_app_with_mode(ApprovalMode::Smart);

    let (tx, _rx) = tokio::sync::oneshot::channel();
    let request = ToolApprovalRequest {
        tool_call_id: "call_t".into(),
        tool_name: "bash".into(),
        arguments: serde_json::json!({}),
        requires_approval: true,
        context: None,
    };
    app.pending_approval = Some((request, tx));

    // Press 'y' to approve
    let key = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
    app.handle_key_event(key);

    assert!(
        app.trust_follow_up.is_some(),
        "trust follow-up should trigger in Smart mode"
    );
    assert_eq!(app.trust_follow_up.as_ref().unwrap().tool_name, "bash");
}

#[tokio::test]
async fn trust_follow_up_not_triggered_in_enabled_mode() {
    let mut app = make_app_with_mode(ApprovalMode::Enabled);

    let (tx, _rx) = tokio::sync::oneshot::channel();
    let request = ToolApprovalRequest {
        tool_call_id: "call_e".into(),
        tool_name: "bash".into(),
        arguments: serde_json::json!({}),
        requires_approval: true,
        context: None,
    };
    app.pending_approval = Some((request, tx));

    let key = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
    app.handle_key_event(key);

    assert!(
        app.trust_follow_up.is_none(),
        "trust follow-up should NOT trigger in Enabled mode"
    );
}

#[tokio::test]
async fn trust_follow_up_not_triggered_in_bypassed_mode() {
    let app = App::new(TuiConfig::default());
    assert!(app.trust_follow_up.is_none());
}

#[tokio::test]
async fn trust_follow_up_y_adds_to_session_trusted() {
    let mut app = App::new(TuiConfig::default());
    app.trust_follow_up = Some(TrustFollowUp {
        tool_name: "bash".to_string(),
        expires_at: Instant::now() + Duration::from_secs(3),
    });

    let key = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
    app.handle_key_event(key);

    assert!(app.session_trusted_tools.contains("bash"));
    assert!(app.trust_follow_up.is_none());
}

#[tokio::test]
async fn trust_follow_up_n_does_not_trust() {
    let mut app = App::new(TuiConfig::default());
    app.trust_follow_up = Some(TrustFollowUp {
        tool_name: "bash".to_string(),
        expires_at: Instant::now() + Duration::from_secs(3),
    });

    let key = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE);
    app.handle_key_event(key);

    assert!(!app.session_trusted_tools.contains("bash"));
    assert!(app.trust_follow_up.is_none());
}

#[test]
fn trust_follow_up_timeout_clears() {
    let mut app = App::new(TuiConfig::default());
    app.trust_follow_up = Some(TrustFollowUp {
        tool_name: "bash".to_string(),
        expires_at: instant_secs_ago(1), // already expired
    });

    app.tick();

    assert!(
        app.trust_follow_up.is_none(),
        "expired trust follow-up should be cleared on tick"
    );
}

#[tokio::test]
async fn trusted_tool_auto_approves_in_smart_mode() {
    let mut app = make_app_with_mode(ApprovalMode::Smart);
    app.session_trusted_tools.insert("bash".to_string());

    let (tx, rx) = tokio::sync::oneshot::channel();
    let request = ToolApprovalRequest {
        tool_call_id: "call_trusted".into(),
        tool_name: "bash".into(),
        arguments: serde_json::json!({}),
        requires_approval: true,
        context: None,
    };

    app.handle_approval_request(request, tx);

    assert!(
        app.pending_approval.is_none(),
        "trusted tool should auto-approve"
    );
    assert_eq!(rx.await.unwrap(), ToolApproval::Approved);
}

#[tokio::test]
async fn trusted_tool_still_prompts_in_enabled_mode() {
    let mut app = make_app_with_mode(ApprovalMode::Enabled);
    app.session_trusted_tools.insert("bash".to_string());

    let (tx, _rx) = tokio::sync::oneshot::channel();
    let request = ToolApprovalRequest {
        tool_call_id: "call_te".into(),
        tool_name: "bash".into(),
        arguments: serde_json::json!({}),
        requires_approval: true,
        context: None,
    };

    app.handle_approval_request(request, tx);

    assert!(
        app.pending_approval.is_some(),
        "Enabled mode should prompt even for trusted tools"
    );
}

#[test]
fn session_trust_not_persisted() {
    let app = App::new(TuiConfig::default());
    assert!(
        app.session_trusted_tools.is_empty(),
        "new App should have no trusted tools"
    );
}

// ─── Tool Classification ──────────────────────────────────────

#[test]
fn requires_approval_default_is_false() {
    let tool = MockReadTool;
    assert!(!tool.requires_approval());
}

#[test]
fn tool_with_requires_approval_true() {
    let tool = MockWriteTool;
    assert!(tool.requires_approval());
}

// ─── Untrust Commands ──────────────────────────────────────────

#[test]
fn untrust_specific_tool_command() {
    use crate::commands::{CommandResult, execute_command};
    match execute_command("#approve untrust bash") {
        CommandResult::UntrustTool(name) => assert_eq!(name, "bash"),
        other => panic!("expected UntrustTool, got {other:?}"),
    }
}

#[test]
fn untrust_all_command() {
    use crate::commands::{CommandResult, execute_command};
    assert!(matches!(
        execute_command("#approve untrust"),
        CommandResult::UntrustAll
    ));
}

#[test]
fn untrust_specific_removes_from_set() {
    let mut app = App::new(TuiConfig::default());
    app.session_trusted_tools.insert("bash".to_string());
    app.session_trusted_tools.insert("write_file".to_string());

    app.session_trusted_tools.remove("bash");

    assert!(!app.session_trusted_tools.contains("bash"));
    assert!(app.session_trusted_tools.contains("write_file"));
}

#[test]
fn untrust_all_clears_set() {
    let mut app = App::new(TuiConfig::default());
    app.session_trusted_tools.insert("bash".to_string());
    app.session_trusted_tools.insert("write_file".to_string());

    app.session_trusted_tools.clear();

    assert!(app.session_trusted_tools.is_empty());
}

// ─── Edge Cases ──────────────────────────────────────────────

#[tokio::test]
async fn trust_follow_up_cleared_on_new_approval() {
    let mut app = make_app_with_mode(ApprovalMode::Smart);
    app.trust_follow_up = Some(TrustFollowUp {
        tool_name: "old_tool".to_string(),
        expires_at: Instant::now() + Duration::from_secs(3),
    });

    let (tx, _rx) = tokio::sync::oneshot::channel();
    let request = ToolApprovalRequest {
        tool_call_id: "call_new".into(),
        tool_name: "new_tool".into(),
        arguments: serde_json::json!({}),
        requires_approval: true,
        context: None,
    };

    app.handle_approval_request(request, tx);

    assert!(
        app.trust_follow_up.is_none(),
        "trust follow-up should be cleared when new approval arrives"
    );
    assert!(app.pending_approval.is_some());
}

#[tokio::test]
async fn concurrent_plan_and_tool_approval_plan_takes_precedence() {
    let mut app = App::new(TuiConfig::default());
    app.pending_plan_approval = true;

    let (tx, _rx) = tokio::sync::oneshot::channel();
    let request = ToolApprovalRequest {
        tool_call_id: "call_c".into(),
        tool_name: "bash".into(),
        arguments: serde_json::json!({}),
        requires_approval: true,
        context: None,
    };
    app.pending_approval = Some((request, tx));

    // Press 'y' — plan approval should take precedence
    let key = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
    app.handle_key_event(key);

    assert!(
        !app.pending_plan_approval,
        "plan approval should be handled"
    );
    assert!(
        app.pending_approval.is_some(),
        "tool approval should not have been handled"
    );
}
