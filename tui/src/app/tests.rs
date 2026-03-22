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
    store
        .save(session_id, &meta, &llm_messages)
        .unwrap();

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

    app.toggle_operating_mode();
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

    let key = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
    app.handle_key_event(key);
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
