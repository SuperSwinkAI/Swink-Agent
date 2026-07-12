use std::sync::Arc;

use tracing::warn;

use crate::loop_::AgentEvent;

use super::{Agent, SubscriptionId};

impl Agent {
    /// Subscribe to agent events. Returns a subscription ID for later removal.
    pub fn subscribe(
        &mut self,
        callback: impl Fn(&AgentEvent) + Send + Sync + 'static,
    ) -> SubscriptionId {
        self.listeners.subscribe(callback)
    }

    /// Remove a subscription. Returns `true` if the subscription existed.
    pub fn unsubscribe(&mut self, id: SubscriptionId) -> bool {
        self.listeners.unsubscribe(id)
    }

    /// Dispatch an event to all listeners, catching panics.
    ///
    /// Any listener that panics is automatically unsubscribed.
    pub(super) fn dispatch_event(&mut self, event: &AgentEvent) {
        self.listeners.dispatch(event);

        self.event_forwarders.retain(|forwarder| {
            let guarded = std::panic::AssertUnwindSafe(|| forwarder(event.clone()));
            if let Err(error) = std::panic::catch_unwind(guarded) {
                warn!("event forwarder panicked: {error:?}");
                return false;
            }
            true
        });
    }

    /// Add an event forwarder at runtime.
    pub fn add_event_forwarder(&mut self, f: impl Fn(AgentEvent) + Send + Sync + 'static) {
        self.event_forwarders.push(Arc::new(f));
    }

    /// Dispatch an external event to all listeners and forwarders.
    ///
    /// Used for cross-agent event forwarding.
    pub fn forward_event(&mut self, event: &AgentEvent) {
        self.dispatch_event(event);
    }

    /// Emit a custom named event to all subscribers and forwarders.
    pub fn emit(&mut self, name: impl Into<String>, payload: serde_json::Value) {
        let event = AgentEvent::Custom(crate::emit::Emission::new(name, payload));
        self.dispatch_event(&event);
    }
}
