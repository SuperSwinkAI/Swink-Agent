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

/// Shared snapshot of the loop's current `context_messages` for pause checkpoints.
///
/// `run_single_turn` drains pending messages into `LoopState.context_messages` and
/// then clears the `PendingMessageSnapshot`. In the window between that drain and
/// the next `TurnEnd` event (which is when `Agent.in_flight_messages` is updated),
/// a concurrent `pause()` call would miss the newly consumed messages. This
/// snapshot is updated immediately after the drain so that `pause()` can read the
/// full loop context, including messages that are already in `context_messages` but
/// not yet reflected in `in_flight_messages`.
#[doc(hidden)]
#[derive(Default)]
pub struct LoopContextSnapshot {
    messages: Mutex<Option<Vec<AgentMessage>>>,
}

impl LoopContextSnapshot {
    /// Overwrite the snapshot with a clone of `messages`.
    pub(crate) fn replace(&self, messages: &[AgentMessage]) {
        let mut guard = self
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = Some(clone_messages(messages));
    }

    /// Return a clone of the current snapshot, or `None` if not yet set.
    pub(crate) fn snapshot(&self) -> Option<Vec<AgentMessage>> {
        self.messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_deref()
            .map(clone_messages)
    }

    /// Clear the snapshot (called when the loop finishes or agent is reset).
    pub(crate) fn clear(&self) {
        *self
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
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
