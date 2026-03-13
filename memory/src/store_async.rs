//! Async session storage trait for non-blocking backends.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;

use swink_agent::AgentMessage;

use crate::meta::SessionMeta;
use crate::store::SessionFilter;

/// A boxed future returned by [`SessionStoreAsync`] methods.
type AsyncResult<'a, T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send + 'a>>;

/// Async session persistence for non-blocking backends (Redis, S3, cloud storage).
pub trait SessionStoreAsync: Send + Sync {
    /// Persist a session asynchronously.
    fn save(
        &self,
        id: &str,
        model: &str,
        system_prompt: &str,
        messages: &[AgentMessage],
    ) -> AsyncResult<'_, ()>;

    /// Load a session by ID asynchronously.
    fn load(&self, id: &str) -> AsyncResult<'_, (SessionMeta, Vec<AgentMessage>)>;

    /// List all saved sessions asynchronously.
    fn list(&self) -> AsyncResult<'_, Vec<SessionMeta>>;

    /// Delete a session by ID asynchronously.
    fn delete(&self, id: &str) -> AsyncResult<'_, ()>;

    /// Generate a new unique session ID.
    fn new_session_id(&self) -> String;

    /// List sessions matching filter, with default in-memory filter.
    fn list_filtered<'a>(
        &'a self,
        filter: &'a SessionFilter,
    ) -> Pin<Box<dyn Future<Output = io::Result<Vec<SessionMeta>>> + Send + 'a>> {
        Box::pin(async move {
            let all = self.list().await?;
            Ok(all.into_iter().filter(|m| filter.matches(m)).collect())
        })
    }
}

/// Adapter that wraps a synchronous [`SessionStore`](crate::store::SessionStore)
/// as a [`SessionStoreAsync`] by running sync methods via `tokio::task::spawn_blocking`.
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

impl<S: crate::store::SessionStore + 'static> SessionStoreAsync for BlockingSessionStore<S> {
    fn save(
        &self,
        id: &str,
        model: &str,
        system_prompt: &str,
        messages: &[AgentMessage],
    ) -> AsyncResult<'_, ()> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        let model = model.to_string();
        let system_prompt = system_prompt.to_string();
        let llm_messages: Vec<swink_agent::LlmMessage> = messages
            .iter()
            .filter_map(|m| match m {
                swink_agent::AgentMessage::Llm(llm) => Some(llm.clone()),
                swink_agent::AgentMessage::Custom(_) => None,
            })
            .collect();
        Box::pin(async move {
            let agent_messages: Vec<AgentMessage> =
                llm_messages.into_iter().map(AgentMessage::Llm).collect();
            tokio::task::spawn_blocking(move || {
                inner.save(&id, &model, &system_prompt, &agent_messages)
            })
            .await
            .map_err(io::Error::other)?
        })
    }

    fn load(&self, id: &str) -> AsyncResult<'_, (SessionMeta, Vec<AgentMessage>)> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || inner.load(&id))
                .await
                .map_err(io::Error::other)?
        })
    }

    fn list(&self) -> AsyncResult<'_, Vec<SessionMeta>> {
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || inner.list())
                .await
                .map_err(io::Error::other)?
        })
    }

    fn delete(&self, id: &str) -> AsyncResult<'_, ()> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || inner.delete(&id))
                .await
                .map_err(io::Error::other)?
        })
    }

    fn new_session_id(&self) -> String {
        self.inner.new_session_id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonl::JsonlSessionStore;
    use crate::store::SessionStore;

    #[tokio::test]
    async fn blocking_session_store_adapter_works() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl_store = JsonlSessionStore::new(dir.path().to_path_buf()).unwrap();
        let id = jsonl_store.new_session_id();

        let async_store = BlockingSessionStore::new(jsonl_store);

        // Verify new_session_id works through the adapter.
        let async_id = async_store.new_session_id();
        assert_eq!(async_id.len(), 15);

        // Save via async adapter.
        let messages: Vec<AgentMessage> = vec![];
        async_store
            .save(&id, "test-model", "Be helpful.", &messages)
            .await
            .unwrap();

        // List via async adapter.
        let sessions = async_store.list().await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);
        assert_eq!(sessions[0].model, "test-model");

        // Load via async adapter.
        let (meta, loaded_messages) = async_store.load(&id).await.unwrap();
        assert_eq!(meta.id, id);
        assert_eq!(meta.model, "test-model");
        assert!(loaded_messages.is_empty());

        // Delete via async adapter.
        async_store.delete(&id).await.unwrap();
        let sessions = async_store.list().await.unwrap();
        assert!(sessions.is_empty());
    }
}
