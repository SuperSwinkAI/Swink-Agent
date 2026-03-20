//! Subscription management for agent event listeners.
//!
//! [`ListenerRegistry`] owns the map of callbacks and dispatches [`AgentEvent`]s
//! to them, catching panics so a single misbehaving subscriber cannot crash the
//! agent.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use tracing::warn;

use crate::loop_::AgentEvent;

// ─── SubscriptionId ──────────────────────────────────────────────────────────

/// Unique identifier for an event subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionId(u64);

impl SubscriptionId {
    /// Allocate the next unique id.
    pub fn next() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

// ─── ListenerFn ──────────────────────────────────────────────────────────────

/// Type alias for a boxed event listener callback.
pub type ListenerFn = Box<dyn Fn(&AgentEvent) + Send + Sync>;

// ─── ListenerRegistry ────────────────────────────────────────────────────────

/// Owns event listener callbacks and dispatches events to them.
///
/// Panicking listeners are automatically removed so a single bad subscriber
/// cannot crash the agent.
pub struct ListenerRegistry {
    listeners: HashMap<SubscriptionId, ListenerFn>,
}

impl ListenerRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            listeners: HashMap::new(),
        }
    }

    /// Register a callback and return its [`SubscriptionId`].
    pub fn subscribe(
        &mut self,
        callback: impl Fn(&AgentEvent) + Send + Sync + 'static,
    ) -> SubscriptionId {
        let id = SubscriptionId::next();
        self.listeners.insert(id, Box::new(callback));
        id
    }

    /// Remove a subscription. Returns `true` if it existed.
    pub fn unsubscribe(&mut self, id: SubscriptionId) -> bool {
        self.listeners.remove(&id).is_some()
    }

    /// Dispatch an event to all listeners, catching panics.
    ///
    /// Any listener that panics is automatically removed to prevent future
    /// disruption.
    pub fn dispatch(&mut self, event: &AgentEvent) {
        let mut panicked = Vec::new();
        for (id, listener) in &self.listeners {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| listener(event)));
            if let Err(e) = result {
                eprintln!("listener panic: {e:?}");
                panicked.push(*id);
            }
        }
        for id in panicked {
            self.listeners.remove(&id);
            warn!("removed panicking listener {id:?}");
        }
    }

    /// Number of currently registered listeners.
    pub fn len(&self) -> usize {
        self.listeners.len()
    }

}
