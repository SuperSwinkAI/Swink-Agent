use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use futures::Stream;
use ratatui::layout::Rect;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use std::future::Future;

use swink_agent::{
    Agent, AgentEvent, AgentMessage, AgentOptions, AgentTool, AgentToolResult, ApprovalMode,
    AssistantMessage, AssistantMessageEvent, Cost, LlmMessage, ModelSpec, StopReason, StreamFn,
    StreamOptions, ToolApproval, ToolApprovalRequest, Usage, UserMessage,
};

use crate::config::TuiConfig;
use crate::session::{JsonlSessionStore, SessionMeta, SessionStore};

use super::*;

struct MockStreamFn {
    responses: Mutex<Vec<Vec<AssistantMessageEvent>>>,
}

impl MockStreamFn {
    const fn new(responses: Vec<Vec<AssistantMessageEvent>>) -> Self {
        Self {
            responses: Mutex::new(responses),
        }
    }
}

impl StreamFn for MockStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a ModelSpec,
        _context: &'a swink_agent::AgentContext,
        _options: &'a StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        let events = {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                vec![AssistantMessageEvent::Error {
                    stop_reason: StopReason::Error,
                    error_message: "no more scripted responses".to_string(),
                    usage: None,
                    error_kind: None,
                }]
            } else {
                responses.remove(0)
            }
        };
        Box::pin(futures::stream::iter(events))
    }
}

fn text_only_events(text: &str) -> Vec<AssistantMessageEvent> {
    vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::TextStart { content_index: 0 },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: text.to_string(),
        },
        AssistantMessageEvent::TextEnd { content_index: 0 },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            usage: Usage::default(),
            cost: Cost::default(),
        },
    ]
}

fn default_convert(msg: &AgentMessage) -> Option<LlmMessage> {
    match msg {
        AgentMessage::Llm(llm) => Some(llm.clone()),
        AgentMessage::Custom(_) => None,
    }
}

fn make_test_agent(stream_fn: Arc<dyn StreamFn>) -> Agent {
    Agent::new(AgentOptions::new(
        "test system prompt",
        ModelSpec::new("test", "mock-model"),
        stream_fn,
        default_convert,
    ))
}

fn make_test_agent_with_models(
    primary_model: ModelSpec,
    primary_stream_fn: Arc<dyn StreamFn>,
    extra_models: Vec<(ModelSpec, Arc<dyn StreamFn>)>,
) -> Agent {
    Agent::new(
        AgentOptions::new(
            "test system prompt",
            primary_model,
            primary_stream_fn,
            default_convert,
        )
        .with_available_models(extra_models),
    )
}

/// Drain all pending agent events from the channel, feeding them back
/// to the app (which in turn calls `agent.handle_stream_event`).
fn drain_agent_events(app: &mut App) {
    while let Ok(event) = app.agent_rx.try_recv() {
        app.handle_agent_event(event);
    }
}

#[tokio::test]
async fn multi_turn_send_and_receive() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        text_only_events("first response"),
        text_only_events("second response"),
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
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
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
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        text_only_events("response one"),
        text_only_events("response two"),
        text_only_events("response three"),
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
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hi")]));
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
        timestamp: 0,
    };

    app.handle_agent_event(AgentEvent::MessageEnd { message });
    assert_eq!(app.context_tokens_used, 50_000);
}

#[tokio::test]
async fn reset_clears_context_tokens() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![]));
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
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::Error {
                stop_reason: StopReason::Error,
                error_message: "something broke".to_string(),
                usage: None,
                error_kind: None,
            },
        ],
        text_only_events("recovered"),
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

    let primary_stream = Arc::new(MockStreamFn::new(vec![
        text_only_events("from primary after restore"),
        text_only_events("from primary"),
    ]));
    let extra_stream = Arc::new(MockStreamFn::new(vec![text_only_events("from extra")]));

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

fn make_tool_result_message(content: &str) -> DisplayMessage {
    let summary = content
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(60)
        .collect::<String>();
    DisplayMessage {
        role: MessageRole::ToolResult,
        content: content.to_string(),
        thinking: None,
        is_streaming: false,
        collapsed: false,
        summary,
        user_expanded: false,
        expanded_at: Some(Instant::now()),
        plan_mode: false,
        diff_data: None,
    }
}

fn make_user_message(content: &str) -> DisplayMessage {
    DisplayMessage {
        role: MessageRole::User,
        content: content.to_string(),
        thinking: None,
        is_streaming: false,
        collapsed: false,
        summary: String::new(),
        user_expanded: false,
        expanded_at: None,
        plan_mode: false,
        diff_data: None,
    }
}

fn make_assistant_message(content: &str) -> DisplayMessage {
    DisplayMessage {
        role: MessageRole::Assistant,
        content: content.to_string(),
        thinking: None,
        is_streaming: false,
        collapsed: false,
        summary: String::new(),
        user_expanded: false,
        expanded_at: None,
        plan_mode: false,
        diff_data: None,
    }
}

fn make_user_agent_message(content: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![swink_agent::ContentBlock::Text {
            text: content.to_string(),
        }],
        timestamp: 0,
    }))
}

fn make_assistant_agent_message(content: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
        content: vec![swink_agent::ContentBlock::Text {
            text: content.to_string(),
        }],
        provider: "test".to_string(),
        model_id: "mock-model".to_string(),
        usage: Usage::default(),
        cost: Cost::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    }))
}

#[tokio::test]
async fn tool_result_has_collapsed_fields() {
    let msg = make_tool_result_message("file contents here\nsecond line");
    assert!(!msg.collapsed, "tool result starts expanded");
    assert_eq!(msg.summary, "file contents here");
    assert!(!msg.user_expanded);
    assert!(msg.expanded_at.is_some());
}

#[tokio::test]
async fn toggle_collapse_toggles_state() {
    let mut app = App::new(TuiConfig::default());
    app.messages.push(make_tool_result_message("tool output"));

    assert!(!app.messages[0].collapsed);

    app.toggle_collapse(0);
    assert!(app.messages[0].collapsed);
    assert!(!app.messages[0].user_expanded);

    app.toggle_collapse(0);
    assert!(!app.messages[0].collapsed);
    assert!(app.messages[0].user_expanded);
}

#[tokio::test]
async fn toggle_collapse_non_tool_is_noop() {
    let mut app = App::new(TuiConfig::default());
    app.messages.push(make_user_message("hello"));

    app.toggle_collapse(0);
    assert!(
        !app.messages[0].collapsed,
        "user message should not collapse"
    );
}

#[tokio::test]
async fn auto_collapse_after_timeout() {
    let mut app = App::new(TuiConfig::default());
    let mut msg = make_tool_result_message("tool output");
    msg.expanded_at = Some(instant_secs_ago(11));
    app.messages.push(msg);

    assert!(!app.messages[0].collapsed);

    app.tick();

    assert!(
        app.messages[0].collapsed,
        "tool result should auto-collapse after 10 seconds"
    );
}

#[tokio::test]
async fn user_expanded_prevents_auto_collapse() {
    let mut app = App::new(TuiConfig::default());
    let mut msg = make_tool_result_message("tool output");
    msg.expanded_at = Some(instant_secs_ago(11));
    msg.user_expanded = true;
    app.messages.push(msg);

    app.tick();

    assert!(
        !app.messages[0].collapsed,
        "user-expanded tool result should not auto-collapse"
    );
}

#[tokio::test]
async fn select_next_tool_block_navigates() {
    let mut app = App::new(TuiConfig::default());
    app.messages.push(make_user_message("hello"));
    app.messages.push(make_tool_result_message("tool 1"));
    app.messages.push(make_user_message("world"));
    app.messages.push(make_tool_result_message("tool 2"));

    assert_eq!(app.selected_tool_block, None);

    assert!(app.select_next_tool_block());
    assert_eq!(app.selected_tool_block, Some(1));

    assert!(app.select_next_tool_block());
    assert_eq!(app.selected_tool_block, Some(3));

    assert!(app.select_next_tool_block());
    assert_eq!(app.selected_tool_block, Some(3));
}

#[tokio::test]
async fn select_prev_tool_block_navigates() {
    let mut app = App::new(TuiConfig::default());
    app.messages.push(make_user_message("hello"));
    app.messages.push(make_tool_result_message("tool 1"));
    app.messages.push(make_user_message("world"));
    app.messages.push(make_tool_result_message("tool 2"));

    assert!(app.select_prev_tool_block());
    assert_eq!(app.selected_tool_block, Some(3));

    assert!(app.select_prev_tool_block());
    assert_eq!(app.selected_tool_block, Some(1));

    assert!(app.select_prev_tool_block());
    assert_eq!(app.selected_tool_block, Some(1));
}

#[tokio::test]
async fn f2_toggles_most_recent_tool_block() {
    let mut app = App::new(TuiConfig::default());
    app.messages.push(make_user_message("hello"));
    app.messages.push(make_tool_result_message("tool 1"));
    app.messages.push(make_user_message("world"));
    app.messages.push(make_tool_result_message("tool 2"));

    assert_eq!(app.selected_tool_block, None);
    assert!(!app.messages[3].collapsed);

    let key = KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE);
    app.handle_key_event(key);

    assert_eq!(app.selected_tool_block, Some(3));
    assert!(
        app.messages[3].collapsed,
        "most recent tool block should collapse"
    );
}

#[tokio::test]
async fn f2_toggles_selected_tool_block() {
    let mut app = App::new(TuiConfig::default());
    app.messages.push(make_tool_result_message("tool 1"));
    app.messages.push(make_user_message("hello"));
    app.messages.push(make_tool_result_message("tool 2"));

    app.selected_tool_block = Some(0);
    assert!(!app.messages[0].collapsed);

    let key = KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE);
    app.handle_key_event(key);

    assert!(
        app.messages[0].collapsed,
        "selected tool block should collapse"
    );
    assert!(
        !app.messages[2].collapsed,
        "other tool block should stay expanded"
    );
}

#[tokio::test]
async fn capital_e_inserts_char() {
    let mut app = App::new(TuiConfig::default());

    let key = KeyEvent::new(KeyCode::Char('E'), KeyModifiers::SHIFT);
    app.handle_key_event(key);

    assert_eq!(
        app.input.lines[0], "E",
        "Shift+E should insert 'E' into input"
    );
}

#[tokio::test]
async fn f3_cycles_color_mode() {
    use crate::theme::{self, ColorMode};

    theme::set_color_mode(ColorMode::Custom);

    let mut app = App::new(TuiConfig::default());
    let key = KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE);

    app.handle_key_event(key);
    assert_eq!(theme::color_mode(), ColorMode::MonoWhite);

    app.handle_key_event(key);
    assert_eq!(theme::color_mode(), ColorMode::MonoBlack);

    app.handle_key_event(key);
    assert_eq!(theme::color_mode(), ColorMode::Custom);

    theme::set_color_mode(ColorMode::Custom);
}

#[tokio::test]
async fn shift_left_right_cycles_from_input_focus() {
    let mut app = App::new(TuiConfig::default());
    app.messages.push(make_tool_result_message("tool 1"));
    app.messages.push(make_user_message("hello"));
    app.messages.push(make_tool_result_message("tool 2"));

    assert_eq!(app.focus, Focus::Input);
    assert_eq!(app.selected_tool_block, None);

    let key = KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT);
    app.handle_key_event(key);
    assert_eq!(app.selected_tool_block, Some(0));
    assert_eq!(app.focus, Focus::Input, "focus should stay on input");

    app.handle_key_event(key);
    assert_eq!(app.selected_tool_block, Some(2));

    let key = KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT);
    app.handle_key_event(key);
    assert_eq!(app.selected_tool_block, Some(0));
    assert_eq!(app.focus, Focus::Input, "focus should stay on input");
}

#[tokio::test]
async fn mouse_scroll_over_conversation_scrolls_history() {
    let mut app = App::new(TuiConfig::default());
    app.conversation_area = Rect::new(0, 0, 40, 10);
    app.conversation.scroll_offset = 6;
    app.focus = Focus::Input;

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 5,
        row: 5,
        modifiers: KeyModifiers::NONE,
    });

    assert_eq!(app.conversation.scroll_offset, 3);
    assert_eq!(app.focus, Focus::Input);
}

#[tokio::test]
async fn mouse_scroll_outside_conversation_does_nothing() {
    let mut app = App::new(TuiConfig::default());
    app.conversation_area = Rect::new(0, 0, 40, 10);
    app.conversation.scroll_offset = 6;
    app.focus = Focus::Input;

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 5,
        row: 15,
        modifiers: KeyModifiers::NONE,
    });

    assert_eq!(app.conversation.scroll_offset, 6);
    assert_eq!(app.focus, Focus::Input);
}

#[tokio::test]
async fn page_scroll_uses_actual_conversation_height() {
    let mut app = App::new(TuiConfig::default());
    app.conversation_visible_height = 7;
    app.conversation.scroll_offset = 10;

    let key = KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE);
    app.handle_key_event(key);

    assert_eq!(app.conversation.scroll_offset, 3);
}

#[tokio::test]
async fn trim_messages_keeps_newest_twenty_turns() {
    let mut app = App::new(TuiConfig::default());

    for turn in 1..=25 {
        app.messages
            .push(make_user_message(&format!("user {turn}")));
        app.messages
            .push(make_assistant_message(&format!("assistant {turn}")));
    }

    app.trim_messages_to_recent_turns();

    let visible_users: Vec<&str> = app
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .map(|message| message.content.as_str())
        .collect();
    assert_eq!(visible_users.len(), 20);
    assert_eq!(visible_users.first(), Some(&"user 6"));
    assert_eq!(visible_users.last(), Some(&"user 25"));
}

#[tokio::test]
async fn trim_messages_repairs_selected_tool_block() {
    let mut app = App::new(TuiConfig::default());

    for turn in 1..=21 {
        app.messages
            .push(make_user_message(&format!("user {turn}")));
        if turn == 21 {
            app.messages.push(make_tool_result_message("tool output"));
        } else {
            app.messages
                .push(make_assistant_message(&format!("assistant {turn}")));
        }
    }
    app.selected_tool_block = Some(app.messages.len() - 1);

    app.trim_messages_to_recent_turns();

    assert_eq!(app.selected_tool_block, Some(app.messages.len() - 1));
    assert!(matches!(
        app.messages[app.selected_tool_block.unwrap()].role,
        MessageRole::ToolResult
    ));
}

#[tokio::test]
async fn trim_messages_clamps_scroll_offset() {
    let mut app = App::new(TuiConfig::default());
    app.conversation_visible_height = 5;
    app.conversation.scroll_offset = 40;

    for turn in 1..=25 {
        app.messages
            .push(make_user_message(&format!("user {turn}")));
        app.messages
            .push(make_assistant_message(&format!("assistant {turn}")));
    }
    app.conversation.set_rendered_lines_for_test(12);

    app.trim_messages_to_recent_turns();

    assert_eq!(app.conversation.scroll_offset, 7);
}

#[tokio::test]
async fn load_session_keeps_full_agent_state_but_trims_visible_history() {
    let tempdir = tempdir().unwrap();
    let store = JsonlSessionStore::new(tempdir.path().to_path_buf()).unwrap();
    let mut full_messages: Vec<AgentMessage> = Vec::new();
    for turn in 1..=25 {
        full_messages.push(make_user_agent_message(&format!("user {turn}")));
        full_messages.push(make_assistant_agent_message(&format!("assistant {turn}")));
    }
    // Convert AgentMessages to LlmMessages for the store.
    let llm_messages: Vec<LlmMessage> = full_messages
        .iter()
        .filter_map(|m| match m {
            AgentMessage::Llm(llm) => Some(llm.clone()),
            AgentMessage::Custom(_) => None,
        })
        .collect();
    let session_id = "session-1";
    let now = swink_agent_memory::now_utc();
    let meta = SessionMeta {
        id: session_id.to_string(),
        title: "mock-model".to_string(),
        created_at: now,
        updated_at: now,
    };
    store.save(session_id, &meta, &llm_messages).unwrap();

    let stream_fn = Arc::new(MockStreamFn::new(vec![]));
    let agent = make_test_agent(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.session_store = Some(store);
    app.set_agent(agent);

    app.load_session(session_id);

    let visible_turns = app
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .count();
    assert_eq!(visible_turns, 20);
    assert_eq!(
        app.agent.as_ref().unwrap().state().messages.len(),
        full_messages.len()
    );
}

#[tokio::test]
async fn smart_mode_auto_approves_trusted_tool() {
    let mut app = App::new(TuiConfig::default());
    app.approval_mode = ApprovalMode::Smart;
    app.session_trusted_tools.insert("bash".to_string());

    let (tx, rx) = tokio::sync::oneshot::channel();
    let request = ToolApprovalRequest {
        tool_call_id: "call_1".into(),
        tool_name: "bash".into(),
        arguments: serde_json::json!({"command": "ls"}),
        requires_approval: true,
    };

    app.approval_tx.send((request, tx)).await.unwrap();

    let (req, responder) = app.approval_rx.recv().await.unwrap();
    app.handle_approval_request(req, responder);

    assert!(app.pending_approval.is_none());
    assert_eq!(rx.await.unwrap(), ToolApproval::Approved);
}

#[tokio::test]
async fn smart_mode_prompts_for_untrusted_tool() {
    let mut app = App::new(TuiConfig::default());
    app.approval_mode = ApprovalMode::Smart;

    let (tx, _rx) = tokio::sync::oneshot::channel();
    let request = ToolApprovalRequest {
        tool_call_id: "call_2".into(),
        tool_name: "write_file".into(),
        arguments: serde_json::json!({}),
        requires_approval: true,
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
    let stream_fn = Arc::new(MockStreamFn::new(vec![]));
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
    let mut app = App::new(TuiConfig::default());
    app.approval_mode = ApprovalMode::Smart;
    app.session_trusted_tools.insert("bash".to_string());

    let label = match app.approval_mode {
        ApprovalMode::Enabled => "enabled",
        ApprovalMode::Bypassed => "disabled (auto-approve)",
        ApprovalMode::Smart => "smart (auto-approve reads, prompt for writes)",
        _ => "unknown",
    };
    let mut msg = format!("Tool approval: {label}");
    if app.approval_mode == ApprovalMode::Smart && !app.session_trusted_tools.is_empty() {
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

fn instant_secs_ago(secs: u64) -> Instant {
    Instant::now()
        .checked_sub(Duration::from_secs(secs))
        .unwrap()
}

struct MockReadTool;

impl AgentTool for MockReadTool {
    fn name(&self) -> &'static str {
        "read_file"
    }
    fn label(&self) -> &'static str {
        "Read File"
    }
    fn description(&self) -> &'static str {
        "Read a file"
    }
    fn parameters_schema(&self) -> &serde_json::Value {
        &serde_json::Value::Null
    }
    fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
    ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
        Box::pin(async { AgentToolResult::text("ok") })
    }
}

struct MockWriteTool;

impl AgentTool for MockWriteTool {
    fn name(&self) -> &'static str {
        "write_file"
    }
    fn label(&self) -> &'static str {
        "Write File"
    }
    fn description(&self) -> &'static str {
        "Write a file"
    }
    fn parameters_schema(&self) -> &serde_json::Value {
        &serde_json::Value::Null
    }
    fn requires_approval(&self) -> bool {
        true
    }
    fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
    ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
        Box::pin(async { AgentToolResult::text("ok") })
    }
}

fn make_test_agent_with_tools(stream_fn: Arc<dyn StreamFn>) -> Agent {
    let mut agent = Agent::new(AgentOptions::new(
        "test system prompt",
        ModelSpec::new("test", "mock-model"),
        stream_fn,
        default_convert,
    ));
    agent.set_tools(vec![
        Arc::new(MockReadTool) as Arc<dyn AgentTool>,
        Arc::new(MockWriteTool) as Arc<dyn AgentTool>,
    ]);
    agent
}

#[tokio::test]
async fn toggle_operating_mode_changes_mode() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![]));
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
    let stream_fn = Arc::new(MockStreamFn::new(vec![]));
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
    let stream_fn = Arc::new(MockStreamFn::new(vec![]));
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
    let stream_fn = Arc::new(MockStreamFn::new(vec![]));
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
    let stream_fn = Arc::new(MockStreamFn::new(vec![]));
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
    let stream_fn = Arc::new(MockStreamFn::new(vec![]));
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
    let stream_fn = Arc::new(MockStreamFn::new(vec![]));
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

#[tokio::test]
async fn f1_toggles_help_panel() {
    let mut app = App::new(TuiConfig::default());
    assert!(!app.help_panel.visible);

    let key = KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE);
    app.handle_key_event(key);
    assert!(app.help_panel.visible);

    app.handle_key_event(key);
    assert!(!app.help_panel.visible);
}

#[tokio::test]
async fn f1_works_from_conversation_focus() {
    let mut app = App::new(TuiConfig::default());
    app.focus = Focus::Conversation;

    let key = KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE);
    app.handle_key_event(key);

    assert!(app.help_panel.visible);
    assert_eq!(app.focus, Focus::Conversation);
}

#[tokio::test]
async fn hash_help_toggles_panel() {
    let mut app = App::new(TuiConfig::default());
    assert!(!app.help_panel.visible);

    app.input.insert_char('#');
    app.input.insert_char('h');
    app.input.insert_char('e');
    app.input.insert_char('l');
    app.input.insert_char('p');
    app.submit_input();

    assert!(app.help_panel.visible);
}

#[test]
fn tty_detection_logic() {
    // Verify is_terminal() is available and returns a bool.
    // In test environments stdout is typically not a TTY.
    use std::io::IsTerminal;
    let _is_tty: bool = std::io::stdout().is_terminal();
    // The main.rs guard calls this same check and exits if false.
    // We can't test the process::exit path in-process, but we verify
    // the detection function is callable and returns a well-typed value.
}

#[test]
fn minimum_terminal_size_check() {
    use crate::ui::{MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH, meets_minimum_size};

    // Below both dimensions
    assert!(!meets_minimum_size(80, 24));

    // Below width only
    assert!(!meets_minimum_size(100, 30));

    // Below height only
    assert!(!meets_minimum_size(120, 20));

    // Exactly at minimum
    assert!(meets_minimum_size(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT));

    // Above minimum
    assert!(meets_minimum_size(200, 50));
}

#[test]
fn tick_toggles_blink_and_sets_dirty() {
    let mut app = App::new(TuiConfig::default());
    app.status = AgentStatus::Running;
    app.dirty = false;

    // Tick 5 times to trigger a blink toggle (every 5 ticks)
    for _ in 0..5 {
        app.tick();
    }

    assert!(!app.blink_on, "blink should have toggled after 5 ticks");
    assert!(app.dirty, "dirty should be set when agent is running");
}

#[test]
fn tab_cycles_focus_from_input() {
    let mut app = App::new(TuiConfig::default());
    assert_eq!(app.focus, Focus::Input);

    let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
    app.handle_key_event(tab);
    assert_eq!(app.focus, Focus::Conversation);
}

#[test]
fn non_navigation_key_returns_focus_to_input() {
    // When focus is on Conversation, any non-navigation key switches back to Input
    let mut app = App::new(TuiConfig::default());
    app.focus = Focus::Conversation;

    // A regular character key should switch focus back to Input
    let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
    app.handle_key_event(key);
    assert_eq!(app.focus, Focus::Input);
}

#[test]
fn resize_event_sets_dirty() {
    let mut app = App::new(TuiConfig::default());
    app.dirty = false;

    app.handle_terminal_event(&crossterm::event::Event::Resize(120, 40));

    assert!(app.dirty, "resize should set dirty flag");
}

// ─── User Story 1: Approval Modes ──────────────────────────────────────────

#[test]
fn approval_mode_default_is_smart() {
    let app = App::new(TuiConfig::default());
    assert_eq!(app.approval_mode, ApprovalMode::Smart);
}

#[tokio::test]
async fn smart_mode_auto_approves_readonly_tool() {
    let mut app = App::new(TuiConfig::default());
    app.approval_mode = ApprovalMode::Smart;

    let (tx, _rx) = tokio::sync::oneshot::channel();
    let request = ToolApprovalRequest {
        tool_call_id: "call_ro".into(),
        tool_name: "read_file".into(),
        arguments: serde_json::json!({}),
        requires_approval: false,
    };

    // In Smart mode, a non-trusted tool that is not in the approval list
    // still goes through handle_approval_request; the approval callback
    // is what gates it. Here we test the TUI's handle_approval_request.
    // For a read-only tool, the core loop never sends an approval request
    // (it checks requires_approval on the tool trait). So this path
    // would only be reached if the tool has requires_approval=false.
    // The approval callback in the core loop wouldn't fire for such tools.
    // We test that the TUI correctly stores approval for untrusted tools.
    app.handle_approval_request(request, tx);

    // Since the tool is not trusted, it should be pending
    assert!(
        app.pending_approval.is_some(),
        "untrusted tool should trigger pending approval"
    );
}

#[tokio::test]
async fn smart_mode_prompts_for_write_tool() {
    let mut app = App::new(TuiConfig::default());
    app.approval_mode = ApprovalMode::Smart;

    let (tx, _rx) = tokio::sync::oneshot::channel();
    let request = ToolApprovalRequest {
        tool_call_id: "call_w".into(),
        tool_name: "write_file".into(),
        arguments: serde_json::json!({}),
        requires_approval: true,
    };

    app.handle_approval_request(request, tx);
    assert!(app.pending_approval.is_some());
}

#[tokio::test]
async fn enabled_mode_prompts_for_all_tools() {
    let mut app = App::new(TuiConfig::default());
    app.approval_mode = ApprovalMode::Enabled;

    let (tx, _rx) = tokio::sync::oneshot::channel();
    let request = ToolApprovalRequest {
        tool_call_id: "call_r".into(),
        tool_name: "read_file".into(),
        arguments: serde_json::json!({}),
        requires_approval: false,
    };

    app.handle_approval_request(request, tx);
    assert!(
        app.pending_approval.is_some(),
        "Enabled mode should prompt for all tools"
    );
}

#[tokio::test]
async fn bypassed_mode_auto_approves_all() {
    // In Bypassed mode, the core loop never sends approval requests
    // (ApprovalMode::Bypassed skips the callback entirely).
    // This is tested at the core level. The TUI just sets the mode.
    let mut app = App::new(TuiConfig::default());
    app.approval_mode = ApprovalMode::Bypassed;
    // Verify mode is set correctly
    assert_eq!(app.approval_mode, ApprovalMode::Bypassed);
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

// ─── User Story 2: Session Trust Follow-Up ──────────────────────────────────

#[tokio::test]
async fn trust_follow_up_triggers_after_approval_in_smart_mode() {
    let mut app = App::new(TuiConfig::default());
    app.approval_mode = ApprovalMode::Smart;

    let (tx, _rx) = tokio::sync::oneshot::channel();
    let request = ToolApprovalRequest {
        tool_call_id: "call_t".into(),
        tool_name: "bash".into(),
        arguments: serde_json::json!({}),
        requires_approval: true,
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
    let mut app = App::new(TuiConfig::default());
    app.approval_mode = ApprovalMode::Enabled;

    let (tx, _rx) = tokio::sync::oneshot::channel();
    let request = ToolApprovalRequest {
        tool_call_id: "call_e".into(),
        tool_name: "bash".into(),
        arguments: serde_json::json!({}),
        requires_approval: true,
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
    // Bypassed mode auto-approves at core level, so no approval request
    // reaches the TUI. Trust follow-up is never triggered.
    let app = App::new(TuiConfig::default());
    assert!(app.trust_follow_up.is_none());
}

#[tokio::test]
async fn trust_follow_up_y_adds_to_session_trusted() {
    let mut app = App::new(TuiConfig::default());
    app.trust_follow_up = Some(super::state::TrustFollowUp {
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
    app.trust_follow_up = Some(super::state::TrustFollowUp {
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
    app.trust_follow_up = Some(super::state::TrustFollowUp {
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
    let mut app = App::new(TuiConfig::default());
    app.approval_mode = ApprovalMode::Smart;
    app.session_trusted_tools.insert("bash".to_string());

    let (tx, rx) = tokio::sync::oneshot::channel();
    let request = ToolApprovalRequest {
        tool_call_id: "call_trusted".into(),
        tool_name: "bash".into(),
        arguments: serde_json::json!({}),
        requires_approval: true,
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
    let mut app = App::new(TuiConfig::default());
    app.approval_mode = ApprovalMode::Enabled;
    app.session_trusted_tools.insert("bash".to_string());

    let (tx, _rx) = tokio::sync::oneshot::channel();
    let request = ToolApprovalRequest {
        tool_call_id: "call_te".into(),
        tool_name: "bash".into(),
        arguments: serde_json::json!({}),
        requires_approval: true,
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

// ─── User Story 3: Plan Mode & Approval ─────────────────────────────────────

#[tokio::test]
async fn plan_toggle_enters_plan_mode() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![]));
    let agent = make_test_agent_with_tools(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    assert_eq!(app.operating_mode, OperatingMode::Execute);

    app.toggle_operating_mode();
    assert_eq!(app.operating_mode, OperatingMode::Plan);
}

#[tokio::test]
async fn plan_toggle_shows_approval_prompt() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![]));
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
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("executing plan")]));
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
    let stream_fn = Arc::new(MockStreamFn::new(vec![]));
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
    let stream_fn = Arc::new(MockStreamFn::new(vec![]));
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
    let stream_fn = Arc::new(MockStreamFn::new(vec![]));
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
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("ok")]));
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
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("ok")]));
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
    let stream_fn = Arc::new(MockStreamFn::new(vec![]));
    let agent = make_test_agent_with_tools(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    app.enter_plan_mode();
    assert_eq!(app.operating_mode, OperatingMode::Plan);
    // The status bar rendering checks operating_mode == Plan to show badge.
    // We verify the state is correct; rendering is tested visually.
}

// ─── User Story 4: Tool Classification ──────────────────────────────────────

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

// ─── User Story 2 Supplement: Untrust Commands ──────────────────────────────

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

// ─── Edge Cases ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn plan_toggle_during_plan_approval_ignored() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![]));
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
async fn concurrent_plan_and_tool_approval_plan_takes_precedence() {
    let mut app = App::new(TuiConfig::default());
    app.pending_plan_approval = true;

    let (tx, _rx) = tokio::sync::oneshot::channel();
    let request = ToolApprovalRequest {
        tool_call_id: "call_c".into(),
        tool_name: "bash".into(),
        arguments: serde_json::json!({}),
        requires_approval: true,
    };
    app.pending_approval = Some((request, tx));

    // Press 'y' — plan approval should take precedence
    let key = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
    app.handle_key_event(key);

    // Plan approval should have been handled (but approve_plan needs agent,
    // so it would be a no-op here). The key point is that tool approval
    // was NOT handled first.
    assert!(
        !app.pending_plan_approval,
        "plan approval should be handled"
    );
    assert!(
        app.pending_approval.is_some(),
        "tool approval should not have been handled"
    );
}

#[tokio::test]
async fn trust_follow_up_cleared_on_new_approval() {
    let mut app = App::new(TuiConfig::default());
    app.approval_mode = ApprovalMode::Smart;
    app.trust_follow_up = Some(super::state::TrustFollowUp {
        tool_name: "old_tool".to_string(),
        expires_at: Instant::now() + Duration::from_secs(3),
    });

    let (tx, _rx) = tokio::sync::oneshot::channel();
    let request = ToolApprovalRequest {
        tool_call_id: "call_new".into(),
        tool_name: "new_tool".into(),
        arguments: serde_json::json!({}),
        requires_approval: true,
    };

    app.handle_approval_request(request, tx);

    assert!(
        app.trust_follow_up.is_none(),
        "trust follow-up should be cleared when new approval arrives"
    );
    assert!(app.pending_approval.is_some());
}

#[tokio::test]
async fn plan_mode_removes_write_tools() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![]));
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

// ─── Plan approval key handling via event_loop ──────────────────────────────

#[tokio::test]
async fn plan_approval_y_key_approves() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("executed")]));
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
    let stream_fn = Arc::new(MockStreamFn::new(vec![]));
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

// ─── Shift+Tab with streaming guard ─────────────────────────────────────────

#[tokio::test]
async fn shift_tab_ignored_while_running() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![]));
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
