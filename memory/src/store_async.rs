//! Async session storage trait for non-blocking backends.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;

use swink_agent::LlmMessage;

use crate::meta::SessionMeta;

/// A boxed future returned by [`AsyncSessionStore`] methods.
type AsyncResult<'a, T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send + 'a>>;

/// Async session persistence for non-blocking backends (Redis, S3, cloud storage).
pub trait AsyncSessionStore: Send + Sync {
    /// Persist a session asynchronously.
    fn save(&self, id: &str, meta: &SessionMeta, messages: &[LlmMessage]) -> AsyncResult<'_, ()>;

    /// Append messages to an existing session asynchronously.
    fn append(&self, id: &str, messages: &[LlmMessage]) -> AsyncResult<'_, ()>;

    /// Load a session by ID asynchronously.
    fn load(&self, id: &str) -> AsyncResult<'_, (SessionMeta, Vec<LlmMessage>)>;

    /// List all saved sessions asynchronously.
    fn list(&self) -> AsyncResult<'_, Vec<SessionMeta>>;

    /// Delete a session by ID asynchronously.
    fn delete(&self, id: &str) -> AsyncResult<'_, ()>;
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

impl<S: crate::store::SessionStore + 'static> AsyncSessionStore for BlockingSessionStore<S> {
    fn save(
        &self,
        id: &str,
        meta: &SessionMeta,
        messages: &[LlmMessage],
    ) -> AsyncResult<'_, ()> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        let meta = meta.clone();
        let messages = messages.to_vec();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || inner.save(&id, &meta, &messages))
                .await
                .map_err(io::Error::other)?
        })
    }

    fn append(&self, id: &str, messages: &[LlmMessage]) -> AsyncResult<'_, ()> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        let messages = messages.to_vec();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || inner.append(&id, &messages))
                .await
                .map_err(io::Error::other)?
        })
    }

    fn load(&self, id: &str) -> AsyncResult<'_, (SessionMeta, Vec<LlmMessage>)> {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonl::JsonlSessionStore;
    use crate::time::now_utc;

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
        };

        // Save via async adapter.
        let messages: Vec<LlmMessage> = vec![];
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
}
