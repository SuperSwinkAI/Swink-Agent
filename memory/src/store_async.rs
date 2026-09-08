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
use crate::search::{SessionHit, SessionSearchOptions};

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

    /// Persist a session transcript plus its state snapshot asynchronously.
    pub fn save_full(
        &self,
        id: &str,
        meta: &SessionMeta,
        messages: &[AgentMessage],
        state: &serde_json::Value,
    ) -> SessionStoreFuture<'_, SessionMeta> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        let meta = meta.clone();
        let messages = clone_messages_for_blocking(messages);
        let state = state.clone();
        spawn_store_call(move || inner.save_full(&id, &meta, &messages, &state))
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
    /// messages are discarded on load because the blocking adapter has no
    /// serialized-wrapper fallback.
    pub fn load(&self, id: &str) -> SessionStoreFuture<'_, (SessionMeta, Vec<AgentMessage>)> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        let registry = self.registry.clone();
        spawn_store_call(move || inner.load(&id, registry.as_deref()))
    }

    /// Load a session transcript plus its state snapshot asynchronously from
    /// one backend-defined read boundary.
    pub fn load_full(
        &self,
        id: &str,
    ) -> SessionStoreFuture<'_, (SessionMeta, Vec<AgentMessage>, Option<serde_json::Value>)> {
        let inner = Arc::clone(&self.inner);
        let id = id.to_string();
        let registry = self.registry.clone();
        spawn_store_call(move || inner.load_full(&id, registry.as_deref()))
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

    /// Search persisted sessions asynchronously.
    pub fn search(
        &self,
        query: &str,
        options: &SessionSearchOptions,
    ) -> SessionStoreFuture<'_, Vec<SessionHit>> {
        let inner = Arc::clone(&self.inner);
        let query = query.to_string();
        let options = options.clone();
        spawn_store_call(move || inner.search(&query, &options))
    }
}

#[cfg(test)]
#[path = "store_async_tests.rs"]
mod tests;
