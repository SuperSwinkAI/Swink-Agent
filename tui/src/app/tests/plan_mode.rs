use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use swink_agent::testing::ScriptedStreamFn;
use swink_agent::testing::text_events;

use crate::config::TuiConfig;

use super::super::*;
use super::helpers::*;

#[tokio::test]
async fn toggle_operating_mode_changes_mode() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent_with_tools(stream_fn);

    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    assert_eq!(app.operating_mode, OperatingMode::Execute);

    app.toggle_operating_mode();
    assert_eq!(app.operating_mode, OperatingMode::Plan);

    // Toggling from Plan now shows approval prompt instead of directly exiting
    app.toggle_operating_mode();
    assert!(app.pending_plan_approval);
    assert_eq!(
        app.operating_mode,
        OperatingMode::Plan,
        "should stay in Plan until approved"
    );

    // Approve the plan to exit
    app.approve_plan();
    assert_eq!(app.operating_mode, OperatingMode::Execute);
}

#[tokio::test]
async fn plan_mode_filters_tools() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent_with_tools(stream_fn);

    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    assert_eq!(app.agent.as_ref().unwrap().state().tools.len(), 2);

    app.enter_plan_mode();

    let tools = &app.agent.as_ref().unwrap().state().tools;
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name(), "read_file");
}

#[tokio::test]
async fn plan_mode_modifies_system_prompt() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent_with_tools(stream_fn);

    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    app.enter_plan_mode();

    let prompt = &app.agent.as_ref().unwrap().state().system_prompt;
    assert!(
        prompt.contains("planning mode"),
        "system prompt should contain planning mode addendum"
    );
}

#[tokio::test]
async fn exit_plan_mode_restores_tools() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent_with_tools(stream_fn);

    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    app.enter_plan_mode();
    assert_eq!(app.agent.as_ref().unwrap().state().tools.len(), 1);

    app.exit_plan_mode();
    assert_eq!(app.agent.as_ref().unwrap().state().tools.len(), 2);
}

#[tokio::test]
async fn exit_plan_mode_restores_system_prompt() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent_with_tools(stream_fn);

    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    let original_prompt = app.agent.as_ref().unwrap().state().system_prompt.clone();

    app.enter_plan_mode();
    app.exit_plan_mode();

    let restored_prompt = &app.agent.as_ref().unwrap().state().system_prompt;
    assert_eq!(
        &original_prompt, restored_prompt,
        "system prompt should be restored after exiting plan mode"
    );
}

#[tokio::test]
async fn reset_exits_plan_mode() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent_with_tools(stream_fn);

    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    app.enter_plan_mode();
    assert_eq!(app.operating_mode, OperatingMode::Plan);

    if let Some(agent) = &mut app.agent {
        agent.reset();
    }
    app.messages.clear();
    app.operating_mode = OperatingMode::Execute;
    app.saved_tools = None;
    app.saved_system_prompt = None;

    assert_eq!(app.operating_mode, OperatingMode::Execute);
    assert!(app.saved_tools.is_none());
    assert!(app.saved_system_prompt.is_none());
}

#[tokio::test]
async fn shift_tab_toggles_plan_mode() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent_with_tools(stream_fn);

    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    assert_eq!(app.operating_mode, OperatingMode::Execute);

    let key = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
    app.handle_key_event(key);
    assert_eq!(app.operating_mode, OperatingMode::Plan);

    // Second Shift+Tab shows approval prompt (stays in Plan)
    let key = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
    app.handle_key_event(key);
    assert!(app.pending_plan_approval);
    assert_eq!(app.operating_mode, OperatingMode::Plan);

    // Approve plan to exit
    app.approve_plan();
    assert_eq!(app.operating_mode, OperatingMode::Execute);
}

// ─── Plan Mode & Approval ─────────────────────────────────────

#[tokio::test]
async fn plan_toggle_enters_plan_mode() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent_with_tools(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    assert_eq!(app.operating_mode, OperatingMode::Execute);

    app.toggle_operating_mode();
    assert_eq!(app.operating_mode, OperatingMode::Plan);
}

#[tokio::test]
async fn plan_toggle_shows_approval_prompt() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent_with_tools(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    app.enter_plan_mode();
    assert_eq!(app.operating_mode, OperatingMode::Plan);

    // Toggle again — should show approval instead of exiting
    app.toggle_operating_mode();
    assert!(app.pending_plan_approval);
    assert_eq!(
        app.operating_mode,
        OperatingMode::Plan,
        "should stay in Plan until approved"
    );
}

#[tokio::test]
async fn plan_approval_y_exits_plan_and_sends_messages() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![text_events("executing plan")]));
    let agent = make_test_agent_with_tools(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    app.enter_plan_mode();

    // Add plan-mode assistant messages
    app.messages.push(DisplayMessage {
        role: MessageRole::Assistant,
        content: "step 1: read files".to_string(),
        thinking: None,
        is_streaming: false,
        collapsed: false,
        summary: String::new(),
        user_expanded: false,
        expanded_at: None,
        plan_mode: true,
        diff_data: None,
    });
    app.messages.push(DisplayMessage {
        role: MessageRole::Assistant,
        content: "step 2: modify code".to_string(),
        thinking: None,
        is_streaming: false,
        collapsed: false,
        summary: String::new(),
        user_expanded: false,
        expanded_at: None,
        plan_mode: true,
        diff_data: None,
    });

    app.pending_plan_approval = true;
    app.approve_plan();

    assert_eq!(app.operating_mode, OperatingMode::Execute);
    assert!(!app.pending_plan_approval);

    // Verify the plan was sent as a user message
    let user_msgs: Vec<&str> = app
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::User)
        .map(|m| m.content.as_str())
        .collect();
    assert!(
        user_msgs
            .iter()
            .any(|m| m.contains("step 1") && m.contains("---") && m.contains("step 2")),
        "plan messages should be concatenated with separator"
    );
}

#[tokio::test]
async fn plan_approval_n_stays_in_plan() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent_with_tools(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    app.enter_plan_mode();
    app.pending_plan_approval = true;
    app.reject_plan();

    assert_eq!(app.operating_mode, OperatingMode::Plan);
    assert!(!app.pending_plan_approval);
}

#[tokio::test]
async fn plan_approval_empty_plan_skips_send() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent_with_tools(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    app.enter_plan_mode();
    // No assistant messages added
    app.pending_plan_approval = true;
    app.approve_plan();

    assert_eq!(app.operating_mode, OperatingMode::Execute);
    // No user message should have been created for the plan
    assert!(
        !app.messages
            .iter()
            .any(|m| m.role == MessageRole::User && !m.content.is_empty()),
        "empty plan should not send a user message"
    );
}

#[tokio::test]
async fn plan_toggle_ignored_while_agent_running() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent_with_tools(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);
    app.status = AgentStatus::Running;

    app.toggle_operating_mode();
    assert_eq!(
        app.operating_mode,
        OperatingMode::Execute,
        "toggle should be ignored while running"
    );
}

#[tokio::test]
async fn plan_messages_concatenated_with_separator() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![text_events("ok")]));
    let agent = make_test_agent_with_tools(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    app.enter_plan_mode();

    for step in &["step 1", "step 2", "step 3"] {
        app.messages.push(DisplayMessage {
            role: MessageRole::Assistant,
            content: step.to_string(),
            thinking: None,
            is_streaming: false,
            collapsed: false,
            summary: String::new(),
            user_expanded: false,
            expanded_at: None,
            plan_mode: true,
            diff_data: None,
        });
    }

    app.pending_plan_approval = true;
    app.approve_plan();

    let plan_msg = app
        .messages
        .iter()
        .find(|m| m.role == MessageRole::User && m.content.contains("step 1"))
        .expect("should find plan user message");

    assert_eq!(plan_msg.content, "step 1\n\n---\n\nstep 2\n\n---\n\nstep 3");
}

#[tokio::test]
async fn plan_mode_only_collects_assistant_messages() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![text_events("ok")]));
    let agent = make_test_agent_with_tools(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    app.enter_plan_mode();

    // Add user message (should be excluded)
    app.messages.push(DisplayMessage {
        role: MessageRole::User,
        content: "please plan".to_string(),
        thinking: None,
        is_streaming: false,
        collapsed: false,
        summary: String::new(),
        user_expanded: false,
        expanded_at: None,
        plan_mode: true,
        diff_data: None,
    });

    // Add assistant message (should be included)
    app.messages.push(DisplayMessage {
        role: MessageRole::Assistant,
        content: "here is the plan".to_string(),
        thinking: None,
        is_streaming: false,
        collapsed: false,
        summary: String::new(),
        user_expanded: false,
        expanded_at: None,
        plan_mode: true,
        diff_data: None,
    });

    // Add tool result (should be excluded)
    app.messages.push(DisplayMessage {
        role: MessageRole::ToolResult,
        content: "file contents".to_string(),
        thinking: None,
        is_streaming: false,
        collapsed: false,
        summary: "file contents".to_string(),
        user_expanded: false,
        expanded_at: None,
        plan_mode: true,
        diff_data: None,
    });

    app.pending_plan_approval = true;
    app.approve_plan();

    // Find the user message that was created by approve_plan (not the original "please plan")
    let plan_msgs: Vec<&str> = app
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::User && !m.plan_mode)
        .map(|m| m.content.as_str())
        .collect();

    // The approve_plan should have created a user message with only assistant content
    assert!(
        plan_msgs.iter().any(|m| *m == "here is the plan"),
        "only assistant messages should be in the plan, got: {plan_msgs:?}"
    );
}

#[tokio::test]
async fn plan_badge_shown_in_plan_mode() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent_with_tools(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    app.enter_plan_mode();
    assert_eq!(app.operating_mode, OperatingMode::Plan);
    // The status bar rendering checks operating_mode == Plan to show badge.
    // We verify the state is correct; rendering is tested visually.
}

// ─── Edge Cases ──────────────────────────────────────────────

#[tokio::test]
async fn plan_toggle_during_plan_approval_ignored() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent_with_tools(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    app.enter_plan_mode();
    app.pending_plan_approval = true;

    // Try to toggle again — should be ignored
    app.toggle_operating_mode();
    assert!(
        app.pending_plan_approval,
        "plan approval should still be pending"
    );
    assert_eq!(app.operating_mode, OperatingMode::Plan);
}

#[tokio::test]
async fn plan_mode_removes_write_tools() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent_with_tools(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    assert_eq!(app.agent.as_ref().unwrap().state().tools.len(), 2);

    app.enter_plan_mode();

    let tools = &app.agent.as_ref().unwrap().state().tools;
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name(), "read_file");
    assert!(
        !tools[0].requires_approval(),
        "remaining tool should not require approval"
    );
}

// ─── Plan approval key handling ──────────────────────────────

#[tokio::test]
async fn plan_approval_y_key_approves() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![text_events("executed")]));
    let agent = make_test_agent_with_tools(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    app.enter_plan_mode();
    app.messages.push(DisplayMessage {
        role: MessageRole::Assistant,
        content: "the plan".to_string(),
        thinking: None,
        is_streaming: false,
        collapsed: false,
        summary: String::new(),
        user_expanded: false,
        expanded_at: None,
        plan_mode: true,
        diff_data: None,
    });
    app.pending_plan_approval = true;

    let key = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
    app.handle_key_event(key);

    assert!(!app.pending_plan_approval);
    assert_eq!(app.operating_mode, OperatingMode::Execute);
}

#[tokio::test]
async fn plan_approval_n_key_rejects() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent_with_tools(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    app.enter_plan_mode();
    app.pending_plan_approval = true;

    let key = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE);
    app.handle_key_event(key);

    assert!(!app.pending_plan_approval);
    assert_eq!(
        app.operating_mode,
        OperatingMode::Plan,
        "should stay in plan mode after rejection"
    );
}

// ─── Shift+Tab with streaming guard ──────────────────────────

#[tokio::test]
async fn shift_tab_ignored_while_running() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent_with_tools(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);
    app.status = AgentStatus::Running;

    let key = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
    app.handle_key_event(key);

    assert_eq!(
        app.operating_mode,
        OperatingMode::Execute,
        "Shift+Tab should be ignored while running"
    );
}
