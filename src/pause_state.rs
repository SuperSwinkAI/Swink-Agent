use std::sync::Mutex;

use crate::types::AgentMessage;

/// Shared snapshot of loop-local pending messages for pause checkpoints.
///
/// The live loop can move queued follow-up or steering messages into its local
/// `LoopState.pending_messages` between turns. `Agent::pause()` runs outside
/// that task, so it needs a synchronized view of the loop-local pending batch
/// in addition to whatever still remains in the shared queues.
#[doc(hidden)]
#[derive(Default)]
pub struct PendingMessageSnapshot {
    pending_messages: Mutex<Vec<AgentMessage>>,
}

impl PendingMessageSnapshot {
    pub(crate) fn replace(&self, pending_messages: &[AgentMessage]) {
        let mut guard = self
            .pending_messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = clone_messages(pending_messages);
    }

    pub(crate) fn append(&self, pending_messages: &[AgentMessage]) {
        let mut guard = self
            .pending_messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.extend(clone_messages(pending_messages));
    }

    pub(crate) fn clear(&self) {
        self.pending_messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    pub(crate) fn snapshot(&self) -> Vec<AgentMessage> {
        clone_messages(
            &self
                .pending_messages
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

fn clone_messages(messages: &[AgentMessage]) -> Vec<AgentMessage> {
    messages
        .iter()
        .filter_map(|message| match message {
            AgentMessage::Llm(llm) => Some(AgentMessage::Llm(llm.clone())),
            AgentMessage::Custom(custom) => custom.clone_box().map(AgentMessage::Custom),
        })
        .collect()
}
