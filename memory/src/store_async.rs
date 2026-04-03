//! Async session storage trait for non-blocking backends.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;

use swink_agent::{AgentMessage, CustomMessageRegistry};

use crate::entry::SessionEntry;
use crate::interrupt::InterruptState;
use crate::load_options::LoadOptions;
use crate::meta::SessionMeta;

/// A boxed future returned by [`AsyncSessionStore`] methods.
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

/// Async session persistence for non-blocking backends (Redis, S3, cloud storage).
///
/// Mirrors [`SessionStore`](crate::store::SessionStore) with async signatures.
/// All save/load methods use [`AgentMessage`] as the canonical message type.
pub trait AsyncSessionStore: Send + Sync {
    /// Persist a session asynchronously, including both LLM and custom messages.
    fn save(
        &self,
        id: &str,
        meta: &SessionMeta,
        messages: &[AgentMessage],
    ) -> SessionStoreFuture<'_, ()>;

    /// Append messages to an existing session asynchronously.
    fn append(&self, id: &str, messages: &[AgentMessage]) -> SessionStoreFuture<'_, ()>;

    /// Load a session by ID asynchronously.
    ///
    /// If `registry` is `Some`, custom messages are deserialized using the
    /// provided registry. If `None`, custom messages are skipped.
    fn load(
        &self,
        id: &str,
        registry: Option<&CustomMessageRegistry>,
    ) -> SessionStoreFuture<'_, (SessionMeta, Vec<AgentMessage>)>;

    /// List all saved sessions asynchronously.
    fn list(&self) -> SessionStoreFuture<'_, Vec<SessionMeta>>;

    /// Delete a session by ID asynchronously.
    fn delete(&self, id: &str) -> SessionStoreFuture<'_, ()>;

    /// Save session state snapshot asynchronously. Default: no-op.
    fn save_state(&self, id: &str, state: &serde_json::Value) -> SessionStoreFuture<'_, ()> {
        let _ = (id, state);
        Box::pin(async { Ok(()) })
    }

    /// Load session state snapshot asynchronously. Default: `None`.
    fn load_state(&self, id: &str) -> SessionStoreFuture<'_, Option<serde_json::Value>> {
        let _ = id;
        Box::pin(async { Ok(None) })
    }

    /// Persist interrupt state for a session asynchronously.
    fn save_interrupt(&self, id: &str, state: &InterruptState) -> SessionStoreFuture<'_, ()>;

    /// Load interrupt state for a session asynchronously.
    fn load_interrupt(&self, id: &str) -> SessionStoreFuture<'_, Option<InterruptState>>;

    /// Clear interrupt state for a session asynchronously.
    fn clear_interrupt(&self, id: &str) -> SessionStoreFuture<'_, ()>;

    /// Load a session with filtering options asynchronously.
    fn load_with_options(
        &self,
        id: &str,
        options: &LoadOptions,
    ) -> SessionStoreFuture<'_, (SessionMeta, Vec<SessionEntry>)>;
}

/// Adapter that wraps a synchronous [`SessionStore`](crate::store::SessionStore)
/// as an [`AsyncSessionStore`] by running sync methods via `tokio::task::spawn_blocking`.
///
/// Concurrent writes to the same session may corrupt the file.
/// Callers are expected to enforce single-writer access.
pub struct BlockingSessionStore<S: crate::store::SessionStore + 'static> {
    inner: Arc<S>,
}

impl<S: crate::store::SessionStore + 'static> BlockingSessionStore<S> {
    /// Create a new blocking adapter wrapping the given session store.
    pub fn new(store: S) -> Self {
        Self {
            inner: Arc::new(store),
        }
    }
}

/// Extract `LlmMessage` variants from a slice of `AgentMessage`, cloning each.
/// Custom messages are filtered out (they are not `Clone` and cannot cross
/// `spawn_blocking` boundaries). The inner `SessionStore::save` will still
/// persist them if the concrete backend supports it — this limitation only
/// applies to the `BlockingSessionStore` adapter.
fn clone_llm_messages(messages: &[AgentMessage]) -> Vec<AgentMessage> {
    messages
        .iter()
        .filter_map(|m| match m {
            AgentMessage::Llm(llm) => Some(AgentMessage::Llm(llm.clone())),
            AgentMessage::Custom(_) => None,
        })
        .collect()
}

impl<S: crate::store::SessionStore + 'static> AsyncSessionStore for BlockingSessionStore<S> {
    fn save(
        &self,
        id: &str,
        meta: &SessionMeta,
        messages: &[AgentMessage],
    ) -> SessionStoreFuture<'_, ()> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        let meta = meta.clone();
        // Clone LLM messages for thread transfer; custom messages are filtered
        // since Box<dyn CustomMessage> is not Clone. Callers requiring full
        // custom message persistence should use the sync store directly.
        let messages = clone_llm_messages(messages);
        spawn_store_call(move || inner.save(&id, &meta, &messages))
    }

    fn append(&self, id: &str, messages: &[AgentMessage]) -> SessionStoreFuture<'_, ()> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        let messages = clone_llm_messages(messages);
        spawn_store_call(move || inner.append(&id, &messages))
    }

    fn load(
        &self,
        id: &str,
        registry: Option<&CustomMessageRegistry>,
    ) -> SessionStoreFuture<'_, (SessionMeta, Vec<AgentMessage>)> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        // CustomMessageRegistry is not Send, so we load without registry from
        // the blocking adapter. Callers needing custom message deserialization
        // should use the sync store directly or a native async backend.
        let _ = registry;
        spawn_store_call(move || inner.load(&id, None))
    }

    fn list(&self) -> SessionStoreFuture<'_, Vec<SessionMeta>> {
        let inner = Arc::clone(&self.inner);
        spawn_store_call(move || inner.list())
    }

    fn delete(&self, id: &str) -> SessionStoreFuture<'_, ()> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        spawn_store_call(move || inner.delete(&id))
    }

    fn save_state(&self, id: &str, state: &serde_json::Value) -> SessionStoreFuture<'_, ()> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        let state = state.clone();
        spawn_store_call(move || inner.save_state(&id, &state))
    }

    fn load_state(&self, id: &str) -> SessionStoreFuture<'_, Option<serde_json::Value>> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        spawn_store_call(move || inner.load_state(&id))
    }

    fn save_interrupt(&self, id: &str, state: &InterruptState) -> SessionStoreFuture<'_, ()> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        let state = state.clone();
        spawn_store_call(move || inner.save_interrupt(&id, &state))
    }

    fn load_interrupt(&self, id: &str) -> SessionStoreFuture<'_, Option<InterruptState>> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        spawn_store_call(move || inner.load_interrupt(&id))
    }

    fn clear_interrupt(&self, id: &str) -> SessionStoreFuture<'_, ()> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        spawn_store_call(move || inner.clear_interrupt(&id))
    }

    fn load_with_options(
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
        let (loaded_meta, loaded_messages) = async_store.load("test_async", None).await.unwrap();
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
}
