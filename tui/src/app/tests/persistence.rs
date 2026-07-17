use std::sync::Arc;

use serde_json::json;
use tempfile::tempdir;

use swink_agent::testing::ScriptedStreamFn;

use crate::config::TuiConfig;
use crate::session::{JsonlSessionStore, SessionMeta, SessionStore};

use super::super::*;
use super::helpers::*;

#[tokio::test]
async fn load_session_restores_error_messages_with_role_and_content() {
    let tempdir = tempdir().unwrap();
    let store = JsonlSessionStore::new(tempdir.path().to_path_buf()).unwrap();

    let messages = vec![
        make_user_agent_message("trigger an error"),
        make_error_assistant_agent_message("rate limit exceeded"),
    ];

    let session_id = "error-session";
    let now = swink_agent_memory::now_utc();
    let meta = SessionMeta::new(session_id, "mock-model", now, now);
    store.save(session_id, &meta, &messages).unwrap();

    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.session.session_store = Some(store);
    app.set_agent(agent);

    app.load_session(session_id).unwrap();

    // Should have: user message, error message, system "Loaded session" message
    let error_msgs: Vec<_> = app
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::Error)
        .collect();
    assert_eq!(error_msgs.len(), 1, "expected one error display message");
    assert_eq!(error_msgs[0].content, "rate limit exceeded");

    // The user message should also be present
    assert_eq!(
        app.view.messages
            .iter()
            .filter(|m| m.role == MessageRole::User)
            .count(),
        1
    );
}

#[tokio::test]
async fn load_session_error_with_text_content_uses_text() {
    // When an error assistant message has both text content and error_message,
    // the text content should be preferred.
    let tempdir = tempdir().unwrap();
    let store = JsonlSessionStore::new(tempdir.path().to_path_buf()).unwrap();

    let error_msg = swink_agent::AgentMessage::Llm(swink_agent::LlmMessage::Assistant(
        swink_agent::AssistantMessage::new(
            vec![swink_agent::ContentBlock::Text {
                text: "partial response before error".to_string(),
            }],
            "test",
            "mock-model",
        )
        .with_stop_reason(swink_agent::StopReason::Error)
        .with_error_message("connection reset")
        .with_timestamp(0),
    ));

    let messages = vec![make_user_agent_message("hello"), error_msg];

    let session_id = "error-with-text";
    let now = swink_agent_memory::now_utc();
    let meta = SessionMeta::new(session_id, "mock-model", now, now);
    store.save(session_id, &meta, &messages).unwrap();

    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.session.session_store = Some(store);
    app.set_agent(agent);

    app.load_session(session_id).unwrap();

    let error_msgs: Vec<_> = app
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::Error)
        .collect();
    assert_eq!(error_msgs.len(), 1);
    // Text content is preferred over error_message when present
    assert_eq!(error_msgs[0].content, "partial response before error");
}

#[tokio::test]
async fn load_session_restores_assistant_thinking_blocks() {
    let tempdir = tempdir().unwrap();
    let store = JsonlSessionStore::new(tempdir.path().to_path_buf()).unwrap();

    let thinking_msg = swink_agent::AgentMessage::Llm(swink_agent::LlmMessage::Assistant(
        swink_agent::AssistantMessage::new(
            vec![
                swink_agent::ContentBlock::Thinking {
                    thinking: "step one\nstep two".to_string(),
                    signature: None,
                },
                swink_agent::ContentBlock::Text {
                    text: "final answer".to_string(),
                },
            ],
            "test",
            "mock-model",
        )
        .with_stop_reason(swink_agent::StopReason::Stop)
        .with_timestamp(0),
    ));

    let messages = vec![make_user_agent_message("hello"), thinking_msg];

    let session_id = "thinking-session";
    let now = swink_agent_memory::now_utc();
    let meta = SessionMeta::new(session_id, "mock-model", now, now);
    store.save(session_id, &meta, &messages).unwrap();

    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.session.session_store = Some(store);
    app.set_agent(agent);

    app.load_session(session_id).unwrap();

    let assistant_msg = app
        .messages
        .iter()
        .find(|m| m.role == MessageRole::Assistant && m.content == "final answer")
        .expect("assistant message should be restored");
    assert_eq!(
        assistant_msg.thinking.as_deref(),
        Some("step one\nstep two")
    );
}

#[tokio::test]
async fn load_session_restores_tool_result_error_role() {
    let tempdir = tempdir().unwrap();
    let store = JsonlSessionStore::new(tempdir.path().to_path_buf()).unwrap();

    let tool_error = swink_agent::AgentMessage::Llm(swink_agent::LlmMessage::ToolResult(
        swink_agent::ToolResultMessage::new(
            "call_1",
            vec![swink_agent::ContentBlock::Text {
                text: "permission denied".to_string(),
            }],
        )
        .with_is_error(true)
        .with_timestamp(0),
    ));

    let messages = vec![make_user_agent_message("hello"), tool_error];

    let session_id = "tool-error-session";
    let now = swink_agent_memory::now_utc();
    let meta = SessionMeta::new(session_id, "mock-model", now, now);
    store.save(session_id, &meta, &messages).unwrap();

    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.session.session_store = Some(store);
    app.set_agent(agent);

    app.load_session(session_id).unwrap();

    let restored = app
        .messages
        .iter()
        .find(|message| message.content == "permission denied")
        .expect("tool result should be restored");
    assert_eq!(restored.role, MessageRole::Error);
    assert_eq!(restored.summary, "permission denied");
    assert!(restored.collapsed);
}

#[tokio::test]
async fn auto_save_persists_session_state_snapshot() {
    let tempdir = tempdir().unwrap();
    let store = JsonlSessionStore::new(tempdir.path().to_path_buf()).unwrap();
    let session_id = "state-save-session";

    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent(stream_fn);
    let mut app = App::new(TuiConfig::default()).with_session_store(store, session_id.to_string());
    app.set_agent(agent);

    if let Some(agent) = &mut app.agent_io.agent {
        agent.set_messages(vec![make_user_agent_message("hello")]);
        agent
            .session_state()
            .write()
            .unwrap()
            .set("cursor", 42_i64)
            .unwrap();
        agent
            .session_state()
            .write()
            .unwrap()
            .set("prefs", json!({ "theme": "amber" }))
            .unwrap();
    }

    app.auto_save_session().unwrap();

    let reload_store = JsonlSessionStore::new(tempdir.path().to_path_buf()).unwrap();
    assert_eq!(
        reload_store.load_state(session_id).unwrap(),
        Some(json!({
            "cursor": 42,
            "prefs": { "theme": "amber" }
        }))
    );
}

#[tokio::test]
async fn load_session_restores_agent_session_state() {
    let tempdir = tempdir().unwrap();
    let store = JsonlSessionStore::new(tempdir.path().to_path_buf()).unwrap();
    let session_id = "state-load-session";
    let now = swink_agent_memory::now_utc();
    let meta = SessionMeta::new(session_id, "mock-model", now, now);

    store
        .save(session_id, &meta, &[make_user_agent_message("hello")])
        .unwrap();
    store
        .save_state(session_id, &json!({ "draft": "keep me", "turn": 3 }))
        .unwrap();

    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent(stream_fn);
    let mut app = App::new(TuiConfig::default());
    app.session.session_store = Some(store);
    app.set_agent(agent);

    app.load_session(session_id).unwrap();

    let agent = app.agent_io.agent.as_ref().unwrap();
    let (draft, turn) = {
        let state = agent.session_state().read().unwrap();
        (state.get::<String>("draft"), state.get::<i64>("turn"))
    };
    assert_eq!(draft, Some("keep me".to_string()));
    assert_eq!(turn, Some(3));
}

#[tokio::test]
async fn load_session_without_saved_state_clears_existing_agent_session_state() {
    let tempdir = tempdir().unwrap();
    let store = JsonlSessionStore::new(tempdir.path().to_path_buf()).unwrap();
    let session_id = "state-clear-session";
    let now = swink_agent_memory::now_utc();
    let meta = SessionMeta::new(session_id, "mock-model", now, now);

    store
        .save(session_id, &meta, &[make_user_agent_message("hello")])
        .unwrap();

    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent(stream_fn);
    agent
        .session_state()
        .write()
        .unwrap()
        .set("stale", true)
        .unwrap();

    let mut app = App::new(TuiConfig::default());
    app.session.session_store = Some(store);
    app.set_agent(agent);

    app.load_session(session_id).unwrap();

    let agent = app.agent_io.agent.as_ref().unwrap();
    let is_empty = {
        let state = agent.session_state().read().unwrap();
        state.is_empty()
    };
    assert!(is_empty, "loading should replace prior session state");
}

#[tokio::test]
async fn load_session_with_corrupted_saved_state_keeps_in_memory_state_and_reports_error() {
    let tempdir = tempdir().unwrap();
    let store = JsonlSessionStore::new(tempdir.path().to_path_buf()).unwrap();
    let session_id = "state-corrupt-session";
    let now = swink_agent_memory::now_utc();
    let meta = SessionMeta::new(session_id, "mock-model", now, now);

    store
        .save(session_id, &meta, &[make_user_agent_message("hello")])
        .unwrap();

    let path = tempdir.path().join(format!("{session_id}.jsonl"));
    let mut contents = std::fs::read_to_string(&path).unwrap();
    contents.push_str("{\"_state\":true,\"data\":\n");
    std::fs::write(&path, contents).unwrap();

    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![]));
    let agent = make_test_agent(stream_fn);
    agent
        .session_state()
        .write()
        .unwrap()
        .set("draft", "keep me")
        .unwrap();

    let mut app = App::new(TuiConfig::default());
    app.session.session_store = Some(store);
    app.set_agent(agent);

    let err = app.load_session(session_id).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

    let agent = app.agent_io.agent.as_ref().unwrap();
    let draft = {
        let state = agent.session_state().read().unwrap();
        state.get::<String>("draft")
    };
    assert_eq!(draft, Some("keep me".to_string()));
    assert!(
        app.view.messages
            .iter()
            .any(|message| message.content.contains("Failed to load session")),
        "load failures should surface a user-visible system message"
    );
}

/// Regression for #1111: the `#key <provider> <api-key>` path must not reach
/// the real OS keychain.
///
/// `App::store_key` calls `credentials::store_credential` directly — there is
/// no `fn`-pointer seam here like the wizard has — so before #1111 this test
/// (and every other App test that submitted a `#key` line) blocked forever
/// inside `SecKeychainFindGenericPassword` waiting on macOS's SecurityAgent.
/// It now resolves against the in-memory fake. Reaching the assertions at all
/// is the substance of this test; see `credentials::tests` for the proof that
/// the backend really is the fake.
#[tokio::test]
async fn hash_key_command_stores_through_the_fake_keychain() {
    let mut app = App::new(TuiConfig::default());

    app.store_key("openai", "sk-app-path-sentinel-1111");

    assert!(
        app.view.messages
            .iter()
            .any(|message| message.content.contains("API key stored for: openai")),
        "store_key should report success via the fake backend, not fail or hang"
    );
    assert!(
        !app.view.messages
            .iter()
            .any(|message| message.content.contains("Failed to store key")),
        "the fake backend must not report a storage failure"
    );
}

/// Regression for #1111: `#keys` (list) reads every provider's credential.
/// That was one live keychain read per provider before the backend seam.
#[tokio::test]
async fn hash_keys_command_reads_through_the_fake_keychain() {
    let mut app = App::new(TuiConfig::default());

    app.list_keys();

    assert!(
        app.view.messages
            .iter()
            .any(|message| message.content.contains("Provider credentials:")),
        "list_keys should render the provider table without touching the keychain"
    );
}
