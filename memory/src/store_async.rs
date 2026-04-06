//! Async adapter over synchronous session stores.
//!
//! This module provides [`BlockingSessionStore`], which wraps any
//! [`SessionStore`](crate::store::SessionStore) and exposes async methods by
//! offloading each call to [`tokio::task::spawn_blocking`].
//!
//! # Why no async trait?
//!
//! A previous version of this module defined `AsyncSessionStore`, a trait that
//! mirrored `SessionStore` with async signatures. That trait was removed because:
//!
//! - It offered no behaviour beyond bridging sync → async via `spawn_blocking`.
//! - Its `load` signature accepted a per-call `registry` argument that
//!   `BlockingSessionStore` could never honour (a `&CustomMessageRegistry`
//!   reference cannot cross a `spawn_blocking` boundary), creating a silent
//!   footgun in the API.
//! - No implementation other than `BlockingSessionStore` existed or was planned.
//!
//! Callers that previously held `Box<dyn AsyncSessionStore>` should use
//! `Arc<BlockingSessionStore<S>>` directly, which is callable from async
//! contexts and has the same method set.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;

use swink_agent::{AgentMessage, CustomMessageRegistry};

use crate::entry::SessionEntry;
use crate::interrupt::InterruptState;
use crate::load_options::LoadOptions;
use crate::meta::SessionMeta;

/// A boxed future returned by [`BlockingSessionStore`] methods.
pub type SessionStoreFuture<'a, T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send + 'a>>;

fn spawn_store_call<T: Send + 'static>(
    f: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> SessionStoreFuture<'static, T> {
    Box::pin(async move {
        tokio::task::spawn_blocking(f)
            .await
            .map_err(io::Error::other)?
    })
}

/// Adapter that wraps a synchronous [`SessionStore`](crate::store::SessionStore)
/// and exposes async methods by running each call via `tokio::task::spawn_blocking`.
///
/// Custom messages are preserved faithfully: `save`/`append` snapshot custom
/// messages to their JSON envelope before crossing the thread boundary, and
/// `load` uses the registry configured at construction time to restore them.
///
/// To restore custom messages on load, provide the registry once at construction
/// via [`BlockingSessionStore::with_registry`].  A `&CustomMessageRegistry`
/// reference cannot cross `spawn_blocking` boundaries, so a per-call registry
/// is not supported.
///
/// Concurrent writes to the same session may corrupt the file.
/// Callers are expected to enforce single-writer access.
pub struct BlockingSessionStore<S: crate::store::SessionStore + 'static> {
    inner: Arc<S>,
    registry: Option<Arc<CustomMessageRegistry>>,
}

impl<S: crate::store::SessionStore + 'static> BlockingSessionStore<S> {
    /// Create a new blocking adapter wrapping the given session store.
    pub fn new(store: S) -> Self {
        Self {
            inner: Arc::new(store),
            registry: None,
        }
    }

    /// Attach a [`CustomMessageRegistry`] for deserializing custom messages on load.
    ///
    /// Because `&CustomMessageRegistry` cannot cross `spawn_blocking` boundaries,
    /// the registry must be provided once at construction rather than per call.
    #[must_use]
    pub fn with_registry(mut self, registry: Arc<CustomMessageRegistry>) -> Self {
        self.registry = Some(registry);
        self
    }
}

/// Clone messages for transfer across `spawn_blocking`.
///
/// Delegates to [`swink_agent::clone_messages_for_send`] which snapshots
/// `Custom` variants into `SerializedCustomMessage` wrappers so they can
/// cross thread boundaries faithfully.
fn clone_messages_for_blocking(messages: &[AgentMessage]) -> Vec<AgentMessage> {
    swink_agent::clone_messages_for_send(messages)
}

impl<S: crate::store::SessionStore + 'static> BlockingSessionStore<S> {
    /// Persist a session asynchronously, including both LLM and custom messages.
    pub fn save(
        &self,
        id: &str,
        meta: &SessionMeta,
        messages: &[AgentMessage],
    ) -> SessionStoreFuture<'_, ()> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        let meta = meta.clone();
        let messages = clone_messages_for_blocking(messages);
        spawn_store_call(move || inner.save(&id, &meta, &messages))
    }

    /// Append messages to an existing session asynchronously.
    pub fn append(&self, id: &str, messages: &[AgentMessage]) -> SessionStoreFuture<'_, ()> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        let messages = clone_messages_for_blocking(messages);
        spawn_store_call(move || inner.append(&id, &messages))
    }

    /// Load a session by ID asynchronously.
    ///
    /// Custom messages are restored using the registry supplied to
    /// [`BlockingSessionStore::with_registry`]. Without a registry, custom
    /// messages are returned as raw `SerializedCustomMessage` wrappers (preserved
    /// but not fully deserialized).
    pub fn load(&self, id: &str) -> SessionStoreFuture<'_, (SessionMeta, Vec<AgentMessage>)> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        let registry = self.registry.clone();
        spawn_store_call(move || inner.load(&id, registry.as_deref()))
    }

    /// List all saved sessions asynchronously.
    pub fn list(&self) -> SessionStoreFuture<'_, Vec<SessionMeta>> {
        let inner = Arc::clone(&self.inner);
        spawn_store_call(move || inner.list())
    }

    /// Delete a session by ID asynchronously.
    pub fn delete(&self, id: &str) -> SessionStoreFuture<'_, ()> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        spawn_store_call(move || inner.delete(&id))
    }

    /// Save session state snapshot asynchronously.
    pub fn save_state(&self, id: &str, state: &serde_json::Value) -> SessionStoreFuture<'_, ()> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        let state = state.clone();
        spawn_store_call(move || inner.save_state(&id, &state))
    }

    /// Load session state snapshot asynchronously. Returns `None` if not set.
    pub fn load_state(&self, id: &str) -> SessionStoreFuture<'_, Option<serde_json::Value>> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        spawn_store_call(move || inner.load_state(&id))
    }

    /// Persist interrupt state for a session asynchronously.
    pub fn save_interrupt(&self, id: &str, state: &InterruptState) -> SessionStoreFuture<'_, ()> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        let state = state.clone();
        spawn_store_call(move || inner.save_interrupt(&id, &state))
    }

    /// Load interrupt state for a session asynchronously.
    pub fn load_interrupt(&self, id: &str) -> SessionStoreFuture<'_, Option<InterruptState>> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        spawn_store_call(move || inner.load_interrupt(&id))
    }

    /// Clear interrupt state for a session asynchronously.
    pub fn clear_interrupt(&self, id: &str) -> SessionStoreFuture<'_, ()> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        spawn_store_call(move || inner.clear_interrupt(&id))
    }

    /// Load a session with filtering options asynchronously.
    pub fn load_with_options(
        &self,
        id: &str,
        options: &LoadOptions,
    ) -> SessionStoreFuture<'_, (SessionMeta, Vec<SessionEntry>)> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        let options = options.clone();
        spawn_store_call(move || inner.load_with_options(&id, &options))
    }
}

#[cfg(test)]
mod tests {
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

        let async_store =
            BlockingSessionStore::new(jsonl_store).with_registry(Arc::clone(&registry));

        let messages: Vec<AgentMessage> = vec![
            AgentMessage::Llm(swink_agent::LlmMessage::User(swink_agent::UserMessage {
                content: vec![swink_agent::ContentBlock::Text {
                    text: "hello".to_string(),
                }],
                timestamp: 1,
                cache_hint: None,
            })),
            AgentMessage::Custom(Box::new(TestCustomMsg {
                data: "preserved".to_string(),
            })),
            AgentMessage::Llm(swink_agent::LlmMessage::User(swink_agent::UserMessage {
                content: vec![swink_agent::ContentBlock::Text {
                    text: "world".to_string(),
                }],
                timestamp: 2,
                cache_hint: None,
            })),
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
    async fn blocking_adapter_append_preserves_custom_messages() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl_store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();
        let registry = Arc::new(test_registry());

        let async_store =
            BlockingSessionStore::new(jsonl_store).with_registry(Arc::clone(&registry));

        // Create session with an LLM message.
        let meta = test_meta("custom_append");
        let initial: Vec<AgentMessage> = vec![AgentMessage::Llm(swink_agent::LlmMessage::User(
            swink_agent::UserMessage {
                content: vec![swink_agent::ContentBlock::Text {
                    text: "start".to_string(),
                }],
                timestamp: 1,
                cache_hint: None,
            },
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
}
