//! Integration tests for session persistence.

use swink_agent::{AgentMessage, ContentBlock, LlmMessage, UserMessage};
use swink_agent_memory::{JsonlSessionStore, SessionMeta, SessionStore};

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: text.to_owned(),
        }],
        timestamp: 0,
    }))
}

#[test]
fn save_and_load_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let id = "test_session_001";
    let model = "test-model";
    let prompt = "Be concise.";
    let messages = vec![user_message("hello"), user_message("world")];

    store.save(id, model, prompt, &messages).unwrap();

    let (meta, loaded) = store.load(id).unwrap();
    assert_eq!(meta.id, id);
    assert_eq!(meta.model, model);
    assert_eq!(meta.system_prompt, prompt);
    assert_eq!(meta.message_count, 2);
    assert_eq!(loaded.len(), 2);
}

#[test]
fn save_empty_session() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let messages: Vec<AgentMessage> = Vec::new();
    store
        .save("empty", "model", "prompt", &messages)
        .unwrap();

    let (meta, loaded) = store.load("empty").unwrap();
    assert_eq!(meta.message_count, 0);
    assert!(loaded.is_empty());
}

#[test]
fn list_sessions_sorted_by_updated_at() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let messages: Vec<AgentMessage> = Vec::new();
    store
        .save("session_old", "model-a", "prompt-a", &messages)
        .unwrap();

    // Manually write a session with a known higher timestamp.
    let meta2 = SessionMeta {
        id: "session_new".to_string(),
        model: "model-b".to_string(),
        system_prompt: "prompt-b".to_string(),
        created_at: 9_999_999_999,
        updated_at: 9_999_999_999,
        message_count: 0,
    };
    let path2 = tmp.path().join("session_new.jsonl");
    let content = serde_json::to_string(&meta2).unwrap() + "\n";
    std::fs::write(&path2, content).unwrap();

    let sessions = store.list().unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].id, "session_new");
    assert_eq!(sessions[1].id, "session_old");
}

#[test]
fn delete_session_removes_file() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let messages: Vec<AgentMessage> = Vec::new();
    store
        .save("to_delete", "model", "prompt", &messages)
        .unwrap();

    assert!(tmp.path().join("to_delete.jsonl").exists());
    store.delete("to_delete").unwrap();
    assert!(!tmp.path().join("to_delete.jsonl").exists());
}

#[test]
fn save_preserves_created_at_on_overwrite() {
    let tmp = tempfile::tempdir().unwrap();
    let store = JsonlSessionStore::new(tmp.path().to_path_buf()).unwrap();

    let messages: Vec<AgentMessage> = Vec::new();
    store.save("rewrite", "model", "prompt", &messages).unwrap();

    let (meta1, _) = store.load("rewrite").unwrap();
    let created_at = meta1.created_at;

    // Save again — created_at should be preserved.
    store.save("rewrite", "model-v2", "new prompt", &messages).unwrap();

    let (meta2, _) = store.load("rewrite").unwrap();
    assert_eq!(meta2.created_at, created_at);
    assert_eq!(meta2.model, "model-v2");
    assert_eq!(meta2.system_prompt, "new prompt");
}
