//! Shared codec for serializing and deserializing [`AgentMessage`] batches.
//!
//! Consolidates the message-envelope logic previously duplicated across
//! checkpoints, JSONL session storage, and blocking async adapters into a
//! single module.
//!
//! ## Provided functionality
//!
//! - [`MessageSlot`] — records the original position of each message in an
//!   interleaved LLM/custom sequence.
//! - [`SerializedMessages`] — the result of splitting a `&[AgentMessage]`
//!   into separate LLM and custom vectors with ordering metadata.
//! - [`serialize_messages`] / [`restore_messages`] — batch serialization and
//!   deserialization with interleaved ordering.
//! - [`restore_single_custom`] — restore one custom-message envelope via a
//!   registry (useful for line-oriented formats like JSONL).
//! - [`SerializedCustomMessage`] — a lightweight [`CustomMessage`](super::CustomMessage)
//!   implementation that holds pre-serialized `type_name` + `to_json` data,
//!   enabling transfer across thread or process boundaries.
//! - [`clone_messages_for_send`] — snapshot a slice of `AgentMessage` into
//!   fully `Send + Clone`-safe values for crossing `spawn_blocking` or IPC.

use serde::{Deserialize, Serialize};

use super::{
    AgentMessage, CustomMessageRegistry, LlmMessage, deserialize_custom_message,
    serialize_custom_message,
};

// ─── MessageSlot ────────────────────────────────────────────────────────────

/// Tracks the original position of each message in the sequence.
///
/// During serialization, LLM and custom messages are stored in separate
/// vectors for backward compatibility. `MessageSlot` records the original
/// ordering so that [`restore_messages`] can reconstruct the interleaved
/// sequence faithfully.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum MessageSlot {
    /// An LLM message at the given index in the `messages` vector.
    Llm { index: usize },
    /// A custom message at the given index in the `custom_messages` vector.
    Custom { index: usize },
}

// ─── SerializedMessages ─────────────────────────────────────────────────────

/// The result of splitting an `AgentMessage` slice into LLM and custom
/// vectors, plus ordering metadata.
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct SerializedMessages {
    /// LLM messages in insertion order.
    pub llm_messages: Vec<LlmMessage>,
    /// Custom message envelopes (`{"type": "…", "data": {…}}`).
    pub custom_messages: Vec<serde_json::Value>,
    /// Records the original interleaved order of LLM and custom messages.
    pub message_order: Vec<MessageSlot>,
}

impl SerializedMessages {
    /// Create a new `SerializedMessages` from its component parts.
    #[must_use]
    pub const fn new(
        llm_messages: Vec<LlmMessage>,
        custom_messages: Vec<serde_json::Value>,
        message_order: Vec<MessageSlot>,
    ) -> Self {
        Self {
            llm_messages,
            custom_messages,
            message_order,
        }
    }
}

// ─── Batch serialize / restore ──────────────────────────────────────────────

/// Split a slice of [`AgentMessage`] into separate LLM and custom vectors
/// with ordering metadata.
///
/// Custom messages that do not support serialization (`type_name()` or
/// `to_json()` returns `None`) are skipped with a `tracing::warn`.
///
/// `kind` is a human-readable label used in log messages (e.g. "checkpoint",
/// "session").
pub fn serialize_messages(messages: &[AgentMessage], kind: &str) -> SerializedMessages {
    let mut llm_messages = Vec::new();
    let mut custom_messages = Vec::new();
    let mut message_order = Vec::new();

    for message in messages {
        match message {
            AgentMessage::Llm(llm) => {
                message_order.push(MessageSlot::Llm {
                    index: llm_messages.len(),
                });
                llm_messages.push(llm.clone());
            }
            AgentMessage::Custom(custom) => {
                if let Some(envelope) = serialize_custom_message(custom.as_ref()) {
                    message_order.push(MessageSlot::Custom {
                        index: custom_messages.len(),
                    });
                    custom_messages.push(envelope);
                } else {
                    tracing::warn!(
                        kind,
                        type_name = custom.type_name().unwrap_or("<unknown>"),
                        "skipping non-serializable CustomMessage"
                    );
                }
            }
        }
    }

    SerializedMessages {
        llm_messages,
        custom_messages,
        message_order,
    }
}

/// Reconstruct an interleaved `Vec<AgentMessage>` from separate LLM and
/// custom vectors, using [`MessageSlot`] ordering metadata.
///
/// If `message_order` is empty (legacy data created before ordering support),
/// falls back to LLM messages first, then custom messages appended.
///
/// If `registry` is `None`, custom messages are silently skipped.
/// Deserialization failures are logged as warnings.
///
/// `kind` is used in log messages (e.g. "checkpoint", "session").
pub fn restore_messages(
    llm_messages: &[LlmMessage],
    custom_messages: &[serde_json::Value],
    message_order: &[MessageSlot],
    registry: Option<&CustomMessageRegistry>,
    kind: &str,
) -> Vec<AgentMessage> {
    if !message_order.is_empty() {
        let mut result = Vec::with_capacity(message_order.len());
        for slot in message_order {
            match slot {
                MessageSlot::Llm { index } => {
                    if let Some(llm) = llm_messages.get(*index) {
                        result.push(AgentMessage::Llm(llm.clone()));
                    }
                }
                MessageSlot::Custom { index } => {
                    if let Some(reg) = registry
                        && let Some(envelope) = custom_messages.get(*index)
                    {
                        match deserialize_custom_message(reg, envelope) {
                            Ok(custom) => result.push(AgentMessage::Custom(custom)),
                            Err(error) => {
                                tracing::warn!(
                                    "failed to deserialize custom message from {kind}: {error}"
                                );
                            }
                        }
                    }
                }
            }
        }
        return result;
    }

    // Legacy fallback: LLM messages first, then custom messages appended.
    let mut result: Vec<AgentMessage> = llm_messages
        .iter()
        .cloned()
        .map(AgentMessage::Llm)
        .collect();

    if let Some(reg) = registry {
        for envelope in custom_messages {
            match deserialize_custom_message(reg, envelope) {
                Ok(custom) => result.push(AgentMessage::Custom(custom)),
                Err(error) => {
                    tracing::warn!("failed to deserialize custom message from {kind}: {error}");
                }
            }
        }
    }

    result
}

// ─── Single-envelope restore ────────────────────────────────────────────────

/// Restore a single custom-message envelope via a registry.
///
/// Returns `Ok(Some(msg))` on success, `Ok(None)` if the registry is `None`,
/// or `Err(reason)` if deserialization fails.
pub fn restore_single_custom(
    registry: Option<&CustomMessageRegistry>,
    envelope: &serde_json::Value,
) -> Result<Option<Box<dyn super::CustomMessage>>, String> {
    registry.map_or_else(
        || Ok(None),
        |reg| deserialize_custom_message(reg, envelope).map(Some),
    )
}

// ─── SerializedCustomMessage ────────────────────────────────────────────────

/// A lightweight [`CustomMessage`](super::CustomMessage) stand-in that holds pre-serialized data.
///
/// Useful for ferrying custom messages across `spawn_blocking` boundaries or
/// other contexts where the original `Box<dyn CustomMessage>` (which is
/// neither `Clone` nor necessarily transferable) must be replaced with a
/// plain-data snapshot.
///
/// Implements `CustomMessage` so it can be stored in `AgentMessage::Custom`
/// and round-trips faithfully through `serialize_custom_message` /
/// `deserialize_custom_message`.
#[derive(Debug, Clone)]
pub struct SerializedCustomMessage {
    name: String,
    json: serde_json::Value,
}

impl SerializedCustomMessage {
    /// Create a new serialized custom message from a name and JSON payload.
    #[must_use]
    pub fn new(name: impl Into<String>, json: serde_json::Value) -> Self {
        Self {
            name: name.into(),
            json,
        }
    }

    /// Attempt to create a `SerializedCustomMessage` from a `dyn CustomMessage`.
    ///
    /// Returns `None` if the custom message does not support serialization.
    #[must_use]
    pub fn from_custom(msg: &dyn super::CustomMessage) -> Option<Self> {
        Some(Self {
            name: msg.type_name()?.to_string(),
            json: msg.to_json()?,
        })
    }
}

impl super::CustomMessage for SerializedCustomMessage {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn type_name(&self) -> Option<&str> {
        Some(&self.name)
    }
    fn to_json(&self) -> Option<serde_json::Value> {
        Some(self.json.clone())
    }
    fn clone_box(&self) -> Option<Box<dyn super::CustomMessage>> {
        Some(Box::new(self.clone()))
    }
}

// ─── clone_messages_for_send ────────────────────────────────────────────────

/// Snapshot a slice of [`AgentMessage`] into fully `Send + Clone`-safe values.
///
/// `Llm` variants are cloned directly. `Custom` variants are
/// snapshot-serialized into [`SerializedCustomMessage`] wrappers so they can
/// cross thread (`spawn_blocking`) or process (IPC) boundaries faithfully.
///
/// Custom messages that lack `type_name()` or `to_json()` are silently
/// dropped — matching the existing behavior of `serialize_custom_message`.
pub fn clone_messages_for_send(messages: &[AgentMessage]) -> Vec<AgentMessage> {
    messages
        .iter()
        .filter_map(|m| match m {
            AgentMessage::Llm(llm) => Some(AgentMessage::Llm(llm.clone())),
            AgentMessage::Custom(custom) => {
                let snapshot = SerializedCustomMessage::from_custom(custom.as_ref())?;
                Some(AgentMessage::Custom(Box::new(snapshot)))
            }
        })
        .collect()
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "message_codec_tests.rs"]
mod tests;
