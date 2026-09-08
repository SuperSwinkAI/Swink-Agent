//! Tests for `store_async`.
#![cfg(test)]

use super::*;
use crate::jsonl::JsonlSessionStore;
use crate::time::now_utc;
use swink_agent::AgentMessage;

#[tokio::test]
async fn blocking_session_store_adapter_works() {
    let dir = tempfile::tempdir().unwrap();
    let jsonl_store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();

    let async_store = BlockingSessionStore::new(jsonl_store);

    let now = now_utc();
    let meta = SessionMeta {
        id: "test_async".to_string(),
        title: "Async test".to_string(),
        created_at: now,
        updated_at: now,
        version: 1,
        sequence: 0,
    };

    // Save via async adapter.
    let messages: Vec<AgentMessage> = vec![];
    async_store
        .save("test_async", &meta, &messages)
        .await
        .unwrap();

    // List via async adapter.
    let sessions = async_store.list().await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "test_async");
    assert_eq!(sessions[0].title, "Async test");

    // Load via async adapter.
    let (loaded_meta, loaded_messages) = async_store.load("test_async").await.unwrap();
    assert_eq!(loaded_meta.id, "test_async");
    assert!(loaded_messages.is_empty());

    // Delete via async adapter.
    async_store.delete("test_async").await.unwrap();
    let sessions = async_store.list().await.unwrap();
    assert!(sessions.is_empty());
}

#[tokio::test]
async fn blocking_adapter_bridges_state_methods() {
    let dir = tempfile::tempdir().unwrap();
    let jsonl_store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();
    let async_store = BlockingSessionStore::new(jsonl_store);

    let now = now_utc();
    let meta = SessionMeta {
        id: "state_async".to_string(),
        title: "State test".to_string(),
        created_at: now,
        updated_at: now,
        version: 1,
        sequence: 0,
    };

    async_store.save("state_async", &meta, &[]).await.unwrap();
    async_store
        .save_state("state_async", &serde_json::json!({"scroll": 42}))
        .await
        .unwrap();

    let state = async_store.load_state("state_async").await.unwrap();
    assert_eq!(state, Some(serde_json::json!({"scroll": 42})));
}

#[tokio::test]
async fn blocking_adapter_bridges_save_full() {
    let dir = tempfile::tempdir().unwrap();
    let jsonl_store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();
    let async_store = BlockingSessionStore::new(jsonl_store);

    let meta = test_meta("full_async");
    let persisted_meta = async_store
        .save_full(
            "full_async",
            &meta,
            &[AgentMessage::Llm(swink_agent::LlmMessage::User(
                swink_agent::UserMessage::new(vec![swink_agent::ContentBlock::Text {
                    text: "hello".to_string(),
                }])
                .with_timestamp(1),
            ))],
            &serde_json::json!({"scroll": 7}),
        )
        .await
        .unwrap();

    assert_eq!(persisted_meta.sequence, 1);
    let (_, loaded_messages) = async_store.load("full_async").await.unwrap();
    assert_eq!(loaded_messages.len(), 1);
    let state = async_store.load_state("full_async").await.unwrap();
    assert_eq!(state, Some(serde_json::json!({"scroll": 7})));
}

#[tokio::test]
async fn blocking_adapter_bridges_load_full() {
    let dir = tempfile::tempdir().unwrap();
    let jsonl_store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();
    let async_store = BlockingSessionStore::new(jsonl_store);

    async_store
        .save_full(
            "full_async_load",
            &test_meta("full_async_load"),
            &[AgentMessage::Llm(swink_agent::LlmMessage::User(
                swink_agent::UserMessage::new(vec![swink_agent::ContentBlock::Text {
                    text: "hello".to_string(),
                }])
                .with_timestamp(1),
            ))],
            &serde_json::json!({"scroll": 11}),
        )
        .await
        .unwrap();

    let (meta, messages, state) = async_store.load_full("full_async_load").await.unwrap();
    assert_eq!(meta.id, "full_async_load");
    assert_eq!(messages.len(), 1);
    assert_eq!(state, Some(serde_json::json!({"scroll": 11})));
}

// ── Helper for custom-message regression tests ──────────────────────

#[derive(Debug)]
struct TestCustomMsg {
    data: String,
}

impl swink_agent::CustomMessage for TestCustomMsg {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn type_name(&self) -> Option<&str> {
        Some("TestCustomMsg")
    }
    fn to_json(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({ "data": self.data }))
    }
}

fn test_registry() -> CustomMessageRegistry {
    let mut registry = CustomMessageRegistry::new();
    registry.register(
        "TestCustomMsg",
        Box::new(|val: serde_json::Value| {
            let data = val
                .get("data")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "missing data".to_string())?;
            Ok(Box::new(TestCustomMsg {
                data: data.to_string(),
            }) as Box<dyn swink_agent::CustomMessage>)
        }),
    );
    registry
}

fn test_meta(id: &str) -> SessionMeta {
    let now = now_utc();
    SessionMeta {
        id: id.to_string(),
        title: "Test".to_string(),
        created_at: now,
        updated_at: now,
        version: 1,
        sequence: 0,
    }
}

// ── Regression tests for #104 ───────────────────────────────────────

#[tokio::test]
async fn blocking_adapter_preserves_custom_messages() {
    let dir = tempfile::tempdir().unwrap();
    let jsonl_store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();
    let registry = Arc::new(test_registry());

    let async_store = BlockingSessionStore::new(jsonl_store).with_registry(Arc::clone(&registry));

    let messages: Vec<AgentMessage> = vec![
        AgentMessage::Llm(swink_agent::LlmMessage::User(
            swink_agent::UserMessage::new(vec![swink_agent::ContentBlock::Text {
                text: "hello".to_string(),
            }])
            .with_timestamp(1),
        )),
        AgentMessage::Custom(Box::new(TestCustomMsg {
            data: "preserved".to_string(),
        })),
        AgentMessage::Llm(swink_agent::LlmMessage::User(
            swink_agent::UserMessage::new(vec![swink_agent::ContentBlock::Text {
                text: "world".to_string(),
            }])
            .with_timestamp(2),
        )),
    ];

    let meta = test_meta("custom_save");
    async_store
        .save("custom_save", &meta, &messages)
        .await
        .unwrap();

    // Load back through the blocking adapter — custom messages must survive.
    let (_, loaded) = async_store.load("custom_save").await.unwrap();
    assert_eq!(loaded.len(), 3, "all three messages must be loaded");
    assert!(matches!(loaded[0], AgentMessage::Llm(_)));
    assert!(matches!(loaded[1], AgentMessage::Custom(_)));
    assert!(matches!(loaded[2], AgentMessage::Llm(_)));

    let custom = loaded[1].downcast_ref::<TestCustomMsg>().unwrap();
    assert_eq!(custom.data, "preserved");
}

#[tokio::test]
async fn blocking_adapter_passes_registry_on_load() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(test_registry());

    // Save directly via the sync store so we know the data is correct.
    {
        let sync_store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();
        let meta = test_meta("reg_load");
        let messages: Vec<AgentMessage> = vec![AgentMessage::Custom(Box::new(TestCustomMsg {
            data: "via-registry".to_string(),
        }))];
        crate::store::SessionStore::save(&sync_store, "reg_load", &meta, &messages).unwrap();
    }

    // Load through the blocking adapter with a registry — must restore the custom message.
    let jsonl_store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();
    let async_store = BlockingSessionStore::new(jsonl_store).with_registry(registry);
    let (_, loaded) = async_store.load("reg_load").await.unwrap();
    assert_eq!(loaded.len(), 1);
    assert!(matches!(loaded[0], AgentMessage::Custom(_)));
    let custom = loaded[0].downcast_ref::<TestCustomMsg>().unwrap();
    assert_eq!(custom.data, "via-registry");
}

#[tokio::test]
async fn blocking_adapter_without_registry_drops_custom_messages_on_load() {
    let dir = tempfile::tempdir().unwrap();

    {
        let sync_store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();
        let meta = test_meta("drop_custom");
        let messages: Vec<AgentMessage> = vec![
            AgentMessage::Llm(swink_agent::LlmMessage::User(
                swink_agent::UserMessage::new(vec![swink_agent::ContentBlock::Text {
                    text: "before".to_string(),
                }])
                .with_timestamp(1),
            )),
            AgentMessage::Custom(Box::new(TestCustomMsg {
                data: "not-restored".to_string(),
            })),
            AgentMessage::Llm(swink_agent::LlmMessage::User(
                swink_agent::UserMessage::new(vec![swink_agent::ContentBlock::Text {
                    text: "after".to_string(),
                }])
                .with_timestamp(2),
            )),
        ];
        crate::store::SessionStore::save(&sync_store, "drop_custom", &meta, &messages).unwrap();
    }

    let jsonl_store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();
    let async_store = BlockingSessionStore::new(jsonl_store);
    let (_, loaded) = async_store.load("drop_custom").await.unwrap();

    assert_eq!(
        loaded.len(),
        2,
        "custom messages should be skipped without a registry"
    );
    assert!(matches!(loaded[0], AgentMessage::Llm(_)));
    assert!(matches!(loaded[1], AgentMessage::Llm(_)));
}

#[tokio::test]
async fn blocking_adapter_append_preserves_custom_messages() {
    let dir = tempfile::tempdir().unwrap();
    let jsonl_store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();
    let registry = Arc::new(test_registry());

    let async_store = BlockingSessionStore::new(jsonl_store).with_registry(Arc::clone(&registry));

    // Create session with an LLM message.
    let meta = test_meta("custom_append");
    let initial: Vec<AgentMessage> = vec![AgentMessage::Llm(swink_agent::LlmMessage::User(
        swink_agent::UserMessage::new(vec![swink_agent::ContentBlock::Text {
            text: "start".to_string(),
        }])
        .with_timestamp(1),
    ))];
    async_store
        .save("custom_append", &meta, &initial)
        .await
        .unwrap();

    // Append a custom message via the blocking adapter.
    let appended: Vec<AgentMessage> = vec![AgentMessage::Custom(Box::new(TestCustomMsg {
        data: "appended".to_string(),
    }))];
    async_store
        .append("custom_append", &appended)
        .await
        .unwrap();

    // Reload and verify the custom message survived.
    let (_, loaded) = async_store.load("custom_append").await.unwrap();
    assert_eq!(loaded.len(), 2);
    assert!(matches!(loaded[0], AgentMessage::Llm(_)));
    assert!(matches!(loaded[1], AgentMessage::Custom(_)));
    let custom = loaded[1].downcast_ref::<TestCustomMsg>().unwrap();
    assert_eq!(custom.data, "appended");
}

/// Verify the store is usable via `Arc` for concurrent async tasks (the
/// typical caller pattern after the `AsyncSessionStore` trait was removed).
#[tokio::test]
async fn arc_blocking_store_usable_concurrently() {
    let dir = tempfile::tempdir().unwrap();
    let jsonl_store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();
    let store = Arc::new(BlockingSessionStore::new(jsonl_store));

    let mut handles = Vec::new();
    for i in 0..3u8 {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            let id = format!("concurrent_{i}");
            let now = now_utc();
            let meta = SessionMeta {
                id: id.clone(),
                title: format!("Session {i}"),
                created_at: now,
                updated_at: now,
                version: 1,
                sequence: 0,
            };
            store.save(&id, &meta, &[]).await.unwrap();
            let (loaded, _) = store.load(&id).await.unwrap();
            assert_eq!(loaded.id, id);
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
}
