use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::message_provider::MessageProvider;
use crate::types::{AgentMessage, LlmMessage};

use super::{Agent, FollowUpMode, SteeringMode};

impl Agent {
    // ── Queue Management ─────────────────────────────────────────────────

    /// Push a steering message into the queue.
    pub fn steer(&mut self, message: AgentMessage) {
        self.steering_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(message);
    }

    /// Push a follow-up message into the queue.
    pub fn follow_up(&mut self, message: AgentMessage) {
        self.follow_up_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(message);
    }

    /// Clear all steering messages.
    pub fn clear_steering(&mut self) {
        self.steering_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// Clear all follow-up messages.
    pub fn clear_follow_up(&mut self) {
        self.follow_up_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// Clear both steering and follow-up queues.
    pub fn clear_queues(&mut self) {
        self.clear_steering();
        self.clear_follow_up();
    }

    /// Returns `true` if there are pending steering or follow-up messages.
    #[must_use]
    pub fn has_pending_messages(&self) -> bool {
        let steering_empty = self
            .steering_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty();
        let follow_up_empty = self
            .follow_up_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty();
        !steering_empty || !follow_up_empty
    }
}

/// [`MessageProvider`] backed by shared steering and follow-up queues.
///
/// Drains messages according to the configured [`SteeringMode`] and
/// [`FollowUpMode`] — either one at a time or all at once.
pub(super) struct QueueMessageProvider {
    pub(super) steering_queue: Arc<Mutex<VecDeque<AgentMessage>>>,
    pub(super) follow_up_queue: Arc<Mutex<VecDeque<AgentMessage>>>,
    pub(super) steering_mode: SteeringMode,
    pub(super) follow_up_mode: FollowUpMode,
}

impl MessageProvider for QueueMessageProvider {
    fn poll_steering(&self) -> Vec<AgentMessage> {
        drain_queue(
            &self.steering_queue,
            self.steering_mode == SteeringMode::OneAtATime,
        )
    }

    fn poll_follow_up(&self) -> Vec<AgentMessage> {
        drain_queue(
            &self.follow_up_queue,
            self.follow_up_mode == FollowUpMode::OneAtATime,
        )
    }
}

pub(super) fn llm_messages_from_queue(
    queue: &Arc<Mutex<VecDeque<AgentMessage>>>,
) -> Vec<LlmMessage> {
    queue
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter_map(|msg| match msg {
            AgentMessage::Llm(llm) => Some(llm.clone()),
            AgentMessage::Custom(_) => None,
        })
        .collect()
}

fn drain_queue(queue: &Mutex<VecDeque<AgentMessage>>, one_at_a_time: bool) -> Vec<AgentMessage> {
    let mut guard = queue
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if guard.is_empty() {
        return Vec::new();
    }
    if one_at_a_time {
        guard.pop_front().into_iter().collect()
    } else {
        guard.drain(..).collect()
    }
}
