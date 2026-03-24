use std::sync::{Arc, Mutex};

use crate::error::AgentError;
use crate::registry::{AgentRef, AgentRegistry};
use crate::types::AgentMessage;

/// A per-agent inbox for receiving messages.
///
/// Wraps a `Vec<AgentMessage>` behind an `Arc<Mutex<_>>` so it can be shared
/// between the agent owner and any senders.
#[derive(Clone)]
pub struct AgentMailbox {
    inbox: Arc<Mutex<Vec<AgentMessage>>>,
}

impl AgentMailbox {
    /// Create an empty mailbox.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inbox: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Push a message into the mailbox.
    pub fn send(&self, message: AgentMessage) {
        self.inbox
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(message);
    }

    /// Take all pending messages, leaving the mailbox empty.
    pub fn drain(&self) -> Vec<AgentMessage> {
        std::mem::take(
            &mut *self
                .inbox
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// Whether the mailbox has pending messages.
    #[must_use]
    pub fn has_messages(&self) -> bool {
        !self
            .inbox
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }

    /// Number of pending messages.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inbox
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Whether the mailbox is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.has_messages()
    }
}

impl Default for AgentMailbox {
    fn default() -> Self {
        Self::new()
    }
}

/// Send a message to a named agent via its steering queue.
///
/// Looks up the agent by name in the registry, acquires the lock, and calls
/// `steer(message)` to inject the message into the agent's steering queue.
///
/// # Errors
///
/// Returns [`AgentError::Plugin`] if the agent is not found in the registry.
pub async fn send_to(
    registry: &AgentRegistry,
    agent_name: &str,
    message: AgentMessage,
) -> Result<(), AgentError> {
    let agent_ref: AgentRef = registry.get(agent_name).ok_or_else(|| {
        AgentError::plugin(
            "messaging",
            std::io::Error::other(format!("agent not found: {agent_name}")),
        )
    })?;
    agent_ref.lock().await.steer(message);
    Ok(())
}
