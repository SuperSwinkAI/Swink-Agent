//! State persistence and checkpointing for agent conversations.
//!
//! Provides a [`Checkpoint`] struct that captures a snapshot of agent state
//! (messages, system prompt, model, turn count, metadata) and a
//! [`CheckpointStore`] trait for async save/load of checkpoints.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::types::message_codec::{self, MessageSlot};
use crate::types::{AgentMessage, Cost, CustomMessageRegistry, LlmMessage, Usage};

mod store;

pub use store::{CheckpointFuture, CheckpointStore};

// â”€â”€â”€ Checkpoint â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// A serializable snapshot of agent conversation state.
///
/// Captures everything needed to restore an agent to a previous point:
/// messages, system prompt, model info, turn count, accumulated usage/cost,
/// and arbitrary metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Unique identifier for this checkpoint.
    pub id: String,
    /// System prompt at the time of the checkpoint.
    pub system_prompt: String,
    /// Model provider name.
    pub provider: String,
    /// Model identifier.
    pub model_id: String,
    /// Conversation messages (LLM messages only).
    pub messages: Vec<LlmMessage>,
    /// Serialized custom messages (envelopes with `type` and `data` fields).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_messages: Vec<serde_json::Value>,
    /// Records the original interleaved order of LLM and custom messages.
    ///
    /// Empty for checkpoints created before ordering support was added;
    /// `restore_messages` falls back to legacy (LLM-first) behavior in that case.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    message_order: Vec<MessageSlot>,
    /// Number of completed turns at the time of checkpointing.
    pub turn_count: usize,
    /// Accumulated token usage.
    pub usage: Usage,
    /// Accumulated cost.
    pub cost: Cost,
    /// Unix timestamp when the checkpoint was created.
    pub created_at: u64,
    /// Arbitrary metadata for application-specific use.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
    /// Serialized session state snapshot (`SessionState.data`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<serde_json::Value>,
}

impl Checkpoint {
    /// Create a new checkpoint from the current agent state.
    ///
    /// Serializes `CustomMessage` variants that support serialization (i.e.
    /// `type_name()` and `to_json()` return `Some`). Custom messages that
    /// cannot be serialized are skipped with a warning.
    ///
    /// Use `with_turn_count()`, `with_usage()`, and `with_cost()` to set
    /// additional fields.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        system_prompt: impl Into<String>,
        provider: impl Into<String>,
        model_id: impl Into<String>,
        messages: &[AgentMessage],
    ) -> Self {
        let serialized = message_codec::serialize_messages(messages, "checkpoint");

        Self {
            id: id.into(),
            system_prompt: system_prompt.into(),
            provider: provider.into(),
            model_id: model_id.into(),
            messages: serialized.llm_messages,
            custom_messages: serialized.custom_messages,
            message_order: serialized.message_order,
            turn_count: 0,
            usage: Usage::default(),
            cost: Cost::default(),
            created_at: crate::util::now_timestamp(),
            metadata: HashMap::new(),
            state: None,
        }
    }

    /// Set the session state snapshot.
    #[must_use]
    pub fn with_state(mut self, state: serde_json::Value) -> Self {
        self.state = Some(state);
        self
    }

    /// Set the turn count.
    #[must_use]
    pub const fn with_turn_count(mut self, turn_count: usize) -> Self {
        self.turn_count = turn_count;
        self
    }

    /// Set the accumulated usage.
    #[must_use]
    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = usage;
        self
    }

    /// Set the accumulated cost.
    #[must_use]
    pub fn with_cost(mut self, cost: Cost) -> Self {
        self.cost = cost;
        self
    }

    /// Add metadata to this checkpoint.
    #[must_use]
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Restore all messages as `AgentMessage` values, preserving their
    /// original interleaved order.
    ///
    /// If `registry` is `None`, custom messages are silently skipped.
    /// Deserialization failures are logged as warnings but do not cause errors.
    ///
    /// For checkpoints created before ordering support, falls back to
    /// legacy behavior (LLM messages first, then custom messages appended).
    #[must_use]
    pub fn restore_messages(&self, registry: Option<&CustomMessageRegistry>) -> Vec<AgentMessage> {
        message_codec::restore_messages(
            &self.messages,
            &self.custom_messages,
            &self.message_order,
            registry,
            "checkpoint",
        )
    }
}

// â”€â”€â”€ LoopCheckpoint â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// A serializable snapshot of the agent loop's in-flight state.
///
/// Captures everything needed to pause a running loop and resume it later:
/// messages, pending injections, system prompt, model, and session state.
/// Created by
/// [`Agent::pause`](crate::Agent::pause) and consumed by
/// [`Agent::resume`](crate::Agent::resume).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopCheckpoint {
    /// All context messages at the time of pause.
    pub messages: Vec<LlmMessage>,
    /// Serialized custom messages (envelopes with `type` and `data` fields).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_messages: Vec<serde_json::Value>,
    /// Records the original interleaved order of LLM and custom messages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    message_order: Vec<MessageSlot>,
    /// Follow-up messages queued for injection into the next turn.
    pub pending_messages: Vec<LlmMessage>,
    /// Serialized custom follow-up messages queued for the next turn.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pending_custom_messages: Vec<serde_json::Value>,
    /// Records the original interleaved order of pending follow-up messages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pending_message_order: Vec<MessageSlot>,
    /// Steering messages queued at the time of pause.
    ///
    /// Older checkpoints without this field deserialize with an empty vec
    /// (backward compatible).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_steering_messages: Vec<LlmMessage>,
    /// Serialized custom steering messages queued at the time of pause.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pending_steering_custom_messages: Vec<serde_json::Value>,
    /// Records the original interleaved order of pending steering messages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pending_steering_message_order: Vec<MessageSlot>,
    /// The system prompt active at the time of pause.
    pub system_prompt: String,
    /// Model provider name.
    pub provider: String,
    /// Model identifier.
    pub model_id: String,
    /// Unix timestamp when the checkpoint was created.
    pub created_at: u64,
    /// Arbitrary metadata for application-specific use.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
    /// Serialized session state snapshot (`SessionState.data`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<serde_json::Value>,
}

impl LoopCheckpoint {
    /// Create a loop checkpoint from the current agent state.
    ///
    /// Serializes `CustomMessage` variants that support serialization.
    /// Non-serializable custom messages are skipped with a warning.
    #[must_use]
    pub fn new(
        system_prompt: impl Into<String>,
        provider: impl Into<String>,
        model_id: impl Into<String>,
        messages: &[AgentMessage],
    ) -> Self {
        let serialized = message_codec::serialize_messages(messages, "loop checkpoint");

        Self {
            messages: serialized.llm_messages,
            custom_messages: serialized.custom_messages,
            message_order: serialized.message_order,
            pending_messages: Vec::new(),
            pending_custom_messages: Vec::new(),
            pending_message_order: Vec::new(),
            pending_steering_messages: Vec::new(),
            pending_steering_custom_messages: Vec::new(),
            pending_steering_message_order: Vec::new(),
            system_prompt: system_prompt.into(),
            provider: provider.into(),
            model_id: model_id.into(),
            created_at: crate::util::now_timestamp(),
            metadata: HashMap::new(),
            state: None,
        }
    }

    /// Set the session state snapshot.
    #[must_use]
    pub fn with_state(mut self, state: serde_json::Value) -> Self {
        self.state = Some(state);
        self
    }

    /// Set pending follow-up messages.
    #[must_use]
    pub fn with_pending_messages(mut self, pending: Vec<LlmMessage>) -> Self {
        self.pending_messages = pending;
        self.pending_custom_messages.clear();
        self.pending_message_order.clear();
        self
    }

    /// Set pending steering messages.
    #[must_use]
    pub fn with_pending_steering_messages(mut self, pending: Vec<LlmMessage>) -> Self {
        self.pending_steering_messages = pending;
        self.pending_steering_custom_messages.clear();
        self.pending_steering_message_order.clear();
        self
    }

    /// Set pending follow-up messages from a full `AgentMessage` batch.
    #[must_use]
    pub fn with_pending_message_batch(mut self, pending: &[AgentMessage]) -> Self {
        let serialized = message_codec::serialize_messages(pending, "loop checkpoint pending");
        self.pending_messages = serialized.llm_messages;
        self.pending_custom_messages = serialized.custom_messages;
        self.pending_message_order = serialized.message_order;
        self
    }

    /// Set pending steering messages from a full `AgentMessage` batch.
    #[must_use]
    pub fn with_pending_steering_message_batch(mut self, pending: &[AgentMessage]) -> Self {
        let serialized =
            message_codec::serialize_messages(pending, "loop checkpoint steering pending");
        self.pending_steering_messages = serialized.llm_messages;
        self.pending_steering_custom_messages = serialized.custom_messages;
        self.pending_steering_message_order = serialized.message_order;
        self
    }

    /// Add metadata.
    #[must_use]
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Restore all messages as `AgentMessage` values, preserving their
    /// original interleaved order.
    ///
    /// If `registry` is `None`, custom messages are silently skipped.
    #[must_use]
    pub fn restore_messages(&self, registry: Option<&CustomMessageRegistry>) -> Vec<AgentMessage> {
        message_codec::restore_messages(
            &self.messages,
            &self.custom_messages,
            &self.message_order,
            registry,
            "loop checkpoint",
        )
    }

    /// Restore pending follow-up messages as `AgentMessage` values.
    #[must_use]
    pub fn restore_pending_messages(
        &self,
        registry: Option<&CustomMessageRegistry>,
    ) -> Vec<AgentMessage> {
        message_codec::restore_messages(
            &self.pending_messages,
            &self.pending_custom_messages,
            &self.pending_message_order,
            registry,
            "loop checkpoint pending",
        )
    }

    /// Restore pending steering messages as `AgentMessage` values.
    #[must_use]
    pub fn restore_pending_steering_messages(
        &self,
        registry: Option<&CustomMessageRegistry>,
    ) -> Vec<AgentMessage> {
        message_codec::restore_messages(
            &self.pending_steering_messages,
            &self.pending_steering_custom_messages,
            &self.pending_steering_message_order,
            registry,
            "loop checkpoint steering pending",
        )
    }

    /// Convert this loop checkpoint into a standard [`Checkpoint`] for storage.
    #[must_use]
    pub fn to_checkpoint(&self, id: impl Into<String>) -> Checkpoint {
        Checkpoint {
            id: id.into(),
            system_prompt: self.system_prompt.clone(),
            provider: self.provider.clone(),
            model_id: self.model_id.clone(),
            messages: self.messages.clone(),
            custom_messages: self.custom_messages.clone(),
            message_order: self.message_order.clone(),
            turn_count: 0,
            usage: Usage::default(),
            cost: Cost::default(),
            created_at: self.created_at,
            metadata: self.metadata.clone(),
            state: self.state.clone(),
        }
    }
}

// â”€â”€â”€ Send + Sync assertions â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Checkpoint>();
    assert_send_sync::<LoopCheckpoint>();
};

// â”€â”€â”€ Tests â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[cfg(test)]
#[path = "checkpoint_tests.rs"]
mod tests;
