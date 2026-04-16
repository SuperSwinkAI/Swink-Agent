use std::sync::Arc;

use swink_agent::testing::ScriptedStreamFn;
use swink_agent::testing::text_events;
use swink_agent::{
    AgentEvent, AgentToolResult, AssistantMessage, AssistantMessageEvent, ContentBlock, Cost,
    LlmMessage, ModelSpec, StopReason, StreamFn, ToolResultMessage, TurnSnapshot, Usage,
};

use crate::config::TuiConfig;

use super::super::*;
use super::helpers::*;

#[tokio::test]
async fn multi_turn_send_and_receive() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![
        text_events("first response"),
        text_events("second response"),
    ]));
    let agent = make_test_agent(stream_fn);

    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    app.send_to_agent("hello".to_string());
    assert_eq!(app.status, AgentStatus::Running);

    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    drain_agent_events(&mut app);

    assert_eq!(
        app.status,
        AgentStatus::Idle,
        "app should be idle after first turn"
    );
    assert!(
        app.messages
            .iter()
            .any(|m| m.role == MessageRole::Assistant && m.content == "first response"),
        "first response should appear in display messages"
    );

    app.send_to_agent("follow up".to_string());
    assert_eq!(app.status, AgentStatus::Running);

    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    drain_agent_events(&mut app);

    assert_eq!(
        app.status,
        AgentStatus::Idle,
        "app should be idle after second turn"
    );
    assert!(
        app.messages
            .iter()
            .any(|m| m.role == MessageRole::Assistant && m.content == "second response"),
        "second response should appear in display messages"
    );
    assert!(
        !app.messages.iter().any(|m| m.role == MessageRole::Error),
        "no error messages should appear during multi-turn"
    );
}

#[tokio::test]
async fn agent_state_transitions_through_events() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![text_events("hello")]));
    let agent = make_test_agent(stream_fn);

    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    assert_eq!(app.status, AgentStatus::Idle);

    app.handle_agent_event(AgentEvent::AgentStart);
    assert_eq!(app.status, AgentStatus::Running);

    app.handle_agent_event(AgentEvent::AgentEnd {
        messages: Arc::new(Vec::new()),
    });
    assert_eq!(app.status, AgentStatus::Idle);

    let agent_ref = app.agent.as_ref().unwrap();
    assert!(
        !agent_ref.state().is_running,
        "agent internal is_running should be false after AgentEnd"
    );
}

#[tokio::test]
async fn three_turn_conversation() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![
        text_events("response one"),
        text_events("response two"),
        text_events("response three"),
    ]));
    let agent = make_test_agent(stream_fn);

    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    for (i, prompt) in ["first", "second", "third"].iter().enumerate() {
        app.send_to_agent(prompt.to_string());
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        drain_agent_events(&mut app);

        assert_eq!(
            app.status,
            AgentStatus::Idle,
            "should be idle after turn {}",
            i + 1
        );
    }

    let assistant_msgs: Vec<&str> = app
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::Assistant)
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(
        assistant_msgs,
        vec!["response one", "response two", "response three"]
    );
    assert!(
        !app.messages.iter().any(|m| m.role == MessageRole::Error),
        "no errors across three turns"
    );
}

#[tokio::test]
async fn message_end_updates_context_tokens_used() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![text_events("hi")]));
    let agent = make_test_agent(stream_fn);

    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    assert_eq!(app.context_budget, 100_000);
    assert_eq!(app.context_tokens_used, 0);

    let message = AssistantMessage {
        content: vec![],
        provider: String::new(),
        model_id: "mock-model".to_string(),
        usage: Usage {
            input: 50_000,
            output: 200,
            cache_read: 0,
            cache_write: 0,
            total: 50_200,
            ..Default::default()
        },
        cost: Cost::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        error_kind: None,
        timestamp: 0,
        cache_hint: None,
    };

    app.handle_agent_event(AgentEvent::MessageEnd { message });
    assert_eq!(app.context_tokens_used, 50_000);
}

#[tokio::test]
async fn reset_clears_context_tokens() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent(stream_fn);

    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);
    app.context_tokens_used = 75_000;

    if let Some(agent) = &mut app.agent {
        agent.reset();
    }
    app.context_tokens_used = 0;
    assert_eq!(app.context_tokens_used, 0);
}

#[tokio::test]
async fn error_response_allows_retry() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![
        vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::Error {
                stop_reason: StopReason::Error,
                error_message: "something broke".to_string(),
                error_kind: None,
                usage: None,
            },
        ],
        text_events("recovered"),
    ]));
    let agent = make_test_agent(stream_fn);

    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    app.send_to_agent("hello".to_string());
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    drain_agent_events(&mut app);

    assert_eq!(
        app.status,
        AgentStatus::Idle,
        "should return to idle even after an error response"
    );
    assert!(
        app.messages
            .iter()
            .any(|m| m.content.contains("something broke")),
        "error response should be visible in the conversation"
    );

    app.send_to_agent("try again".to_string());
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    drain_agent_events(&mut app);

    assert_eq!(app.status, AgentStatus::Idle);
    assert!(
        app.messages
            .iter()
            .any(|m| m.role == MessageRole::Assistant && m.content == "recovered"),
        "recovery response should appear"
    );
}

#[tokio::test]
async fn cycle_model_applies_and_restores_provider_binding_on_send() {
    let primary_model = ModelSpec::new("anthropic", "primary-model");
    let extra_model = ModelSpec::new("openai", "extra-model");

    let primary_stream = Arc::new(ScriptedStreamFn::new(vec![
        text_events("from primary after restore"),
        text_events("from primary"),
    ]));
    let extra_stream = Arc::new(ScriptedStreamFn::new(vec![text_events("from extra")]));

    let agent = make_test_agent_with_models(
        primary_model.clone(),
        primary_stream as Arc<dyn StreamFn>,
        vec![(extra_model.clone(), extra_stream as Arc<dyn StreamFn>)],
    );

    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    assert_eq!(app.model_name, primary_model.model_id);

    app.cycle_model();
    assert_eq!(app.model_name, extra_model.model_id);
    assert_eq!(app.pending_model, Some(extra_model.clone()));

    app.send_to_agent("hello extra".to_string());
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    drain_agent_events(&mut app);

    assert!(
        app.messages
            .iter()
            .any(|m| m.role == MessageRole::Assistant && m.content == "from extra")
    );
    assert_eq!(app.model_name, extra_model.model_id);

    app.cycle_model();
    assert_eq!(app.model_name, primary_model.model_id);
    assert_eq!(app.pending_model, Some(primary_model.clone()));

    app.send_to_agent("hello primary".to_string());
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    drain_agent_events(&mut app);

    assert!(
        app.messages
            .iter()
            .any(|m| m.role == MessageRole::Assistant && m.content == "from primary after restore")
    );
    assert_eq!(app.model_name, primary_model.model_id);
}

#[tokio::test]
async fn turn_end_renders_tool_results_with_diff_data_and_trims_old_turns() {
    let mut app = App::new(TuiConfig::default());

    for turn in 1..=21 {
        app.messages
            .push(make_user_message(&format!("user {turn}")));
        app.messages
            .push(make_assistant_message(&format!("assistant {turn}")));
    }

    let long_first_line = "x".repeat(70);
    let tool_content = format!("{long_first_line}\nsecond line");

    app.handle_agent_event(AgentEvent::TurnEnd {
        assistant_message: AssistantMessage {
            content: vec![ContentBlock::Text {
                text: "done".to_string(),
            }],
            provider: "test".to_string(),
            model_id: "mock-model".to_string(),
            usage: Usage::default(),
            cost: Cost::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            error_kind: None,
            timestamp: 0,
            cache_hint: None,
        },
        tool_results: vec![ToolResultMessage {
            tool_call_id: "call_1".to_string(),
            content: vec![ContentBlock::Text {
                text: tool_content.clone(),
            }],
            is_error: false,
            timestamp: 0,
            details: serde_json::json!({
                "path": "src/lib.rs",
                "is_new_file": false,
                "old_content": "before\nvalue",
                "new_content": "after\nvalue",
            }),
            cache_hint: None,
        }],
        reason: swink_agent::TurnEndReason::ToolsExecuted,
        snapshot: TurnSnapshot {
            turn_index: 20,
            messages: Arc::new(vec![LlmMessage::Assistant(AssistantMessage {
                content: vec![ContentBlock::Text {
                    text: "done".to_string(),
                }],
                provider: "test".to_string(),
                model_id: "mock-model".to_string(),
                usage: Usage::default(),
                cost: Cost::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                error_kind: None,
                timestamp: 0,
                cache_hint: None,
            })]),
            usage: Usage::default(),
            cost: Cost::default(),
            stop_reason: StopReason::Stop,
            state_delta: None,
        },
    });

    let visible_users: Vec<_> = app
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .map(|message| message.content.as_str())
        .collect();
    assert_eq!(visible_users.len(), 20);
    assert_eq!(visible_users.first(), Some(&"user 2"));
    assert_eq!(visible_users.last(), Some(&"user 21"));

    let tool_result = app
        .messages
        .iter()
        .find(|message| message.role == MessageRole::ToolResult)
        .expect("TurnEnd should append a tool result row");
    assert_eq!(tool_result.content, tool_content);
    assert_eq!(tool_result.summary, "x".repeat(60));
    assert!(
        tool_result.expanded_at.is_some(),
        "tool results should start expanded"
    );

    let diff = tool_result
        .diff_data
        .as_ref()
        .expect("tool result should parse diff metadata");
    assert_eq!(diff.path, "src/lib.rs");
    assert!(!diff.is_new_file);
    assert_eq!(diff.old_content, "before\nvalue");
    assert_eq!(diff.new_content, "after\nvalue");
}

#[test]
fn tool_execution_update_surfaces_live_tool_output() {
    let mut app = App::new(TuiConfig::default());

    app.handle_agent_event(AgentEvent::ToolExecutionStart {
        id: "call_1".to_string(),
        name: "bash".to_string(),
        arguments: serde_json::json!({
            "command": "cargo test -p swink-agent-tui",
        }),
    });
    app.handle_agent_event(AgentEvent::ToolExecutionUpdate {
        id: "call_1".to_string(),
        name: "bash".to_string(),
        partial: AgentToolResult::text("Compiling swink-agent-tui\n"),
    });
    app.handle_agent_event(AgentEvent::ToolExecutionUpdate {
        id: "call_1".to_string(),
        name: "bash".to_string(),
        partial: AgentToolResult::text("Finished test profile"),
    });

    assert_eq!(app.tool_panel.active.len(), 1);
    assert_eq!(
        app.tool_panel.active[0].streamed_output,
        "Compiling swink-agent-tui\nFinished test profile"
    );
}
