use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use tempfile::tempdir;

use swink_agent::testing::ScriptedStreamFn;
use swink_agent::{AgentEvent, ModelSpec, ThinkingLevel};

use crate::config::TuiConfig;
use crate::session::{JsonlSessionStore, SessionMeta, SessionStore};

use super::super::*;
use super::helpers::*;

/// Guards tests that depend on the global `COLOR_MODE` atomic from running in parallel.
static COLOR_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[tokio::test]
async fn capital_e_inserts_char() {
    let mut app = App::new(TuiConfig::default());

    let key = KeyEvent::new(KeyCode::Char('E'), KeyModifiers::SHIFT);
    app.handle_key_event(key);

    assert_eq!(
        app.input.lines()[0],
        "E",
        "Shift+E should insert 'E' into input"
    );
}

#[tokio::test]
async fn f3_cycles_color_mode() {
    use crate::theme::{self, ColorMode};

    let _guard = COLOR_TEST_LOCK.lock().unwrap();

    let mut app = App::new(TuiConfig::default());

    // Set color mode AFTER App::new(), since construction resets the global from config.
    theme::set_color_mode(ColorMode::Custom);

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
    let mut full_messages: Vec<swink_agent::AgentMessage> = Vec::new();
    for turn in 1..=25 {
        full_messages.push(make_user_agent_message(&format!("user {turn}")));
        full_messages.push(make_assistant_agent_message(&format!("assistant {turn}")));
    }
    let session_id = "session-1";
    let now = swink_agent_memory::now_utc();
    let meta = SessionMeta {
        id: session_id.to_string(),
        title: "mock-model".to_string(),
        created_at: now,
        updated_at: now,
        version: 1,
        sequence: 0,
    };
    store.save(session_id, &meta, &full_messages).unwrap();

    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.session_store = Some(store);
    app.set_agent(agent);

    app.load_session(session_id).unwrap();

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

    // Blink toggles every 23 ticks (~759ms at 33ms tick cadence)
    for _ in 0..23 {
        app.tick();
    }

    assert!(!app.blink_on, "blink should have toggled after 23 ticks");
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

#[test]
fn with_session_store_overrides_default_store_and_id() {
    let tempdir = tempdir().unwrap();
    let store = JsonlSessionStore::new(tempdir.path().to_path_buf()).unwrap();
    let app =
        App::new(TuiConfig::default()).with_session_store(store, "tui_chat_custom-id".to_string());
    assert_eq!(app.session_id, "tui_chat_custom-id");
    assert!(app.session_store.is_some());
}

#[tokio::test]
async fn resume_into_loads_existing_session() {
    let tempdir = tempdir().unwrap();
    let store = JsonlSessionStore::new(tempdir.path().to_path_buf()).unwrap();

    let session_id = "session-resume-test";
    let now = swink_agent_memory::now_utc();
    let meta = SessionMeta {
        id: session_id.to_string(),
        title: "mock-model".to_string(),
        created_at: now,
        updated_at: now,
        version: 1,
        sequence: 0,
    };
    let messages = vec![
        make_user_agent_message("hello"),
        make_assistant_agent_message("world"),
    ];
    store.save(session_id, &meta, &messages).unwrap();

    let store2 = JsonlSessionStore::new(tempdir.path().to_path_buf()).unwrap();
    let mut app = App::new(TuiConfig::default()).with_session_store(store2, "new-id".to_string());
    let result = app.resume_into(session_id);
    assert!(
        result.is_ok(),
        "resume_into should succeed for existing session"
    );
    assert_eq!(
        app.session_id, session_id,
        "session_id updated after resume"
    );
}

#[tokio::test]
async fn repeated_auto_save_preserves_created_at_and_advances_sequence() {
    // Regression for #196: the TUI rebuilt SessionMeta from scratch on every
    // save, sending sequence=0 each time, which tripped the JSONL store's
    // optimistic-concurrency check and silently dropped every save after the
    // first. `created_at` was also regenerated on each write.
    let tempdir = tempdir().unwrap();
    let store = JsonlSessionStore::new(tempdir.path().to_path_buf()).unwrap();
    let session_id = "session-repeated-save";

    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent(stream_fn);
    let mut app = App::new(TuiConfig::default()).with_session_store(store, session_id.to_string());
    app.set_agent(agent);

    // Populate at least one message so the save has content to persist.
    if let Some(ref mut agent) = app.agent {
        agent.set_messages(vec![make_user_agent_message("hello")]);
    }

    // First save: auto-save writes transcript and session-state together, so
    // the store sequence advances once.
    app.auto_save_session().expect("first save should succeed");
    let first_meta = app.session_meta.clone().expect("meta set after save");
    let created_at = first_meta.created_at;
    assert_eq!(first_meta.sequence, 1, "local sequence mirrors the store");

    // Second save: previously failed silently with a sequence conflict.
    if let Some(ref mut agent) = app.agent {
        agent.set_messages(vec![
            make_user_agent_message("hello"),
            make_assistant_agent_message("world"),
        ]);
    }
    app.auto_save_session()
        .expect("second save should succeed without sequence conflict");
    let second_meta = app.session_meta.clone().unwrap();
    assert_eq!(second_meta.sequence, 2, "sequence advanced on second save");
    assert_eq!(
        second_meta.created_at, created_at,
        "created_at is preserved across saves"
    );

    // Third save — make sure advancement keeps working.
    app.auto_save_session().expect("third save should succeed");
    assert_eq!(app.session_meta.as_ref().unwrap().sequence, 3);

    // Reload from disk and confirm the persisted state matches what we expected.
    let reload_store = JsonlSessionStore::new(tempdir.path().to_path_buf()).unwrap();
    let (persisted_meta, persisted_messages) = reload_store.load(session_id, None).unwrap();
    assert_eq!(persisted_meta.sequence, 3);
    assert_eq!(persisted_meta.created_at, created_at);
    assert_eq!(
        persisted_messages.len(),
        2,
        "latest messages were persisted"
    );
}

#[tokio::test]
async fn auto_save_after_load_preserves_created_at_and_continues_sequence() {
    // Regression for #196: after loading an existing session, saves should
    // carry the loaded meta forward rather than starting at sequence 0.
    let tempdir = tempdir().unwrap();
    let store = JsonlSessionStore::new(tempdir.path().to_path_buf()).unwrap();
    let session_id = "session-load-then-save";

    let original_created = swink_agent_memory::now_utc();
    let meta = SessionMeta {
        id: session_id.to_string(),
        title: "mock-model".to_string(),
        created_at: original_created,
        updated_at: original_created,
        version: 1,
        sequence: 0,
    };
    store
        .save(session_id, &meta, &[make_user_agent_message("hi")])
        .unwrap();
    // After that save the stored sequence is 1.

    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent(stream_fn);
    let store2 = JsonlSessionStore::new(tempdir.path().to_path_buf()).unwrap();
    let mut app =
        App::new(TuiConfig::default()).with_session_store(store2, "placeholder".to_string());
    app.set_agent(agent);
    app.load_session(session_id).unwrap();

    let loaded_meta = app.session_meta.clone().expect("meta set after load");
    assert_eq!(loaded_meta.sequence, 1);
    assert_eq!(loaded_meta.created_at, original_created);

    // Mutate and save — combined transcript+state write advances sequence once.
    if let Some(ref mut agent) = app.agent {
        agent.set_messages(vec![
            make_user_agent_message("hi"),
            make_assistant_agent_message("there"),
        ]);
    }
    app.auto_save_session()
        .expect("save after load should succeed");
    assert_eq!(app.session_meta.as_ref().unwrap().sequence, 2);
    assert_eq!(
        app.session_meta.as_ref().unwrap().created_at,
        original_created,
        "loaded created_at is preserved on subsequent saves"
    );
}

#[tokio::test]
async fn resume_into_errors_on_missing_session() {
    let tempdir = tempdir().unwrap();
    let store = JsonlSessionStore::new(tempdir.path().to_path_buf()).unwrap();
    let mut app = App::new(TuiConfig::default()).with_session_store(store, "new-id".to_string());
    let result = app.resume_into("nonexistent-session");
    assert!(
        result.is_err(),
        "resume_into should error for missing session"
    );
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

#[tokio::test]
async fn slash_thinking_updates_agent_model_and_display_flag() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);

    type_input(&mut app, "/thinking medium");
    app.submit_input();

    assert_eq!(
        app.agent.as_ref().unwrap().state().model.thinking_level,
        ThinkingLevel::Medium
    );
    assert!(app.config.show_thinking);
    assert!(
        app.messages
            .last()
            .is_some_and(|msg| msg.content.contains("Medium"))
    );

    type_input(&mut app, "/thinking off");
    app.submit_input();

    assert_eq!(
        app.agent.as_ref().unwrap().state().model.thinking_level,
        ThinkingLevel::Off
    );
    assert!(!app.config.show_thinking);
}

#[tokio::test]
async fn slash_thinking_updates_pending_model_before_next_send() {
    let primary_model = ModelSpec::new("anthropic", "primary-model");
    let extra_model = ModelSpec::new("openai", "extra-model");
    let primary_stream = Arc::new(ScriptedStreamFn::new(vec![]));
    let extra_stream = Arc::new(ScriptedStreamFn::new(vec![]));

    let agent = make_test_agent_with_models(
        primary_model,
        primary_stream,
        vec![(extra_model, extra_stream)],
    );
    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);
    app.cycle_model();

    type_input(&mut app, "/thinking high");
    app.submit_input();

    assert_eq!(
        app.pending_model.as_ref().map(|model| model.thinking_level),
        Some(ThinkingLevel::High)
    );
    assert_eq!(
        app.available_models[app.model_index].thinking_level,
        ThinkingLevel::High
    );
}

/// Helper: type a literal string into the input editor.
fn type_input(app: &mut App, s: &str) {
    for c in s.chars() {
        app.input.insert_char(c);
    }
}

/// Regression: secrets submitted via `#key <provider> <api-key>` must NOT
/// be recallable via Up-arrow history navigation. Covers issue #614.
#[tokio::test]
async fn hash_key_submission_is_not_recallable_via_history() {
    let mut app = App::new(TuiConfig::default());

    type_input(&mut app, "#key openai sk-leak-sentinel-xyz");
    app.submit_input();

    // Navigate history up — nothing should come back.
    app.input.history_prev();

    assert_eq!(
        app.input.lines(),
        &[String::new()],
        "sensitive `#key` submission must not be recallable via history"
    );
    for line in app.input.lines() {
        assert!(
            !line.contains("sk-leak-sentinel-xyz"),
            "secret value leaked into recalled history line: {line}"
        );
    }
}

/// Regression: multi-line submissions containing a `#key` line must be
/// withheld from history in full — not just the key line.
#[tokio::test]
async fn multiline_hash_key_submission_is_fully_redacted() {
    let mut app = App::new(TuiConfig::default());

    type_input(&mut app, "preamble");
    app.input.insert_newline();
    type_input(&mut app, "#key anthropic sk-ant-top-secret-multi");
    app.input.insert_newline();
    type_input(&mut app, "epilogue");

    app.submit_input();

    app.input.history_prev();

    for line in app.input.lines() {
        assert!(
            !line.contains("sk-ant-top-secret-multi"),
            "multi-line sensitive entry leaked into history: {line}"
        );
    }
    assert_eq!(
        app.input.lines(),
        &[String::new()],
        "multi-line sensitive entry must be fully absent from history"
    );
}

/// Regression: secret-bearing submissions that do not parse as a single
/// `#key <provider> <api-key>` command must fail closed instead of being echoed
/// into the transcript or forwarded as plain user text.
#[tokio::test]
async fn malformed_sensitive_submission_is_blocked_without_user_echo() {
    let mut app = App::new(TuiConfig::default());

    type_input(&mut app, "preamble");
    app.input.insert_newline();
    type_input(&mut app, "#key\topenai\t sk-leak-sentinel-block");
    app.input.insert_newline();
    type_input(&mut app, "epilogue");

    app.submit_input();

    assert!(
        !app.messages.iter().any(|message| {
            message.role == MessageRole::User && message.content.contains("sk-leak-sentinel-block")
        }),
        "malformed sensitive input must not be echoed as a user message"
    );
    assert!(
        app.messages.iter().any(|message| {
            message.role == MessageRole::System
                && message
                    .content
                    .contains("Blocked secret-bearing input that did not parse")
        }),
        "malformed sensitive input should produce a blocking system message"
    );
}

/// Regression: non-sensitive commands must continue to enter history so
/// users can recall them with Up-arrow.
#[tokio::test]
async fn non_sensitive_command_is_recallable_via_history() {
    let mut app = App::new(TuiConfig::default());

    type_input(&mut app, "/help");
    app.submit_input();

    app.input.history_prev();

    assert_eq!(
        app.input.lines(),
        &["/help".to_string()],
        "non-sensitive command should remain recallable via history"
    );
}

/// Regression: plain text submissions continue to enter history.
#[tokio::test]
async fn plain_text_submission_is_recallable_via_history() {
    let mut app = App::new(TuiConfig::default());

    type_input(&mut app, "hello world");
    app.submit_input();

    app.input.history_prev();

    assert_eq!(
        app.input.lines(),
        &["hello world".to_string()],
        "plain text should remain recallable via history"
    );
}

#[tokio::test]
async fn editor_style_submission_queues_once_while_running() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent(stream_fn);

    let mut app = App::new(TuiConfig::default());
    app.set_agent(agent);
    app.status = AgentStatus::Running;

    app.submit_user_text("queued from editor".to_string());

    assert!(
        !app.messages
            .iter()
            .any(|message| message.role == MessageRole::User
                && message.content == "queued from editor"),
        "running submission should stay queued until MessageStart promotion"
    );
    assert_eq!(app.pending_steered, vec!["queued from editor".to_string()]);

    app.handle_agent_event(AgentEvent::MessageStart);

    assert_eq!(
        app.messages
            .iter()
            .filter(|message| {
                message.role == MessageRole::User && message.content == "queued from editor"
            })
            .count(),
        1,
        "queued submission should be promoted into the transcript exactly once"
    );
    assert!(
        app.messages
            .last()
            .is_some_and(|message| message.role == MessageRole::Assistant && message.is_streaming),
        "assistant streaming placeholder should still be added after promotion"
    );
    assert!(
        app.pending_steered.is_empty(),
        "queued submission should be drained after MessageStart promotion"
    );
}

#[test]
fn copy_code_extracts_all_fenced_blocks_from_last_assistant_message() {
    let mut app = App::new(TuiConfig::default());
    app.messages.push(make_assistant_message(
        "Intro\n```rust\nlet first = 1;\n```\ntext\n```json\n{\"second\":2}\n```",
    ));

    let copied = app
        .messages
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::Assistant)
        .and_then(|message| super::super::render_helpers::extract_code_blocks(&message.content));

    assert_eq!(copied, Some("let first = 1;\n\n{\"second\":2}".to_string()));
}
