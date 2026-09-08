//! Rich session entry types for persisting non-message events.
//!
//! [`SessionEntry`] is a discriminated union of all entry types that can be
//! stored in a session JSONL file. Only the [`SessionEntry::Message`] variant
//! is sent to the LLM — all other variants are audit/display metadata.

use serde::{Deserialize, Serialize};
use swink_agent::{LlmMessage, ModelSpec};

/// A single entry in a persisted session.
///
/// Serialized with an adjacently-tagged representation: `{"entry_type": "...", "data": {...}}`.
/// Old-format lines (raw `LlmMessage` without `entry_type`) are deserialized as
/// [`SessionEntry::Message`] via a custom fallback in [`SessionEntry::parse`].
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "entry_type", content = "data", rename_all = "snake_case")]
pub enum SessionEntry {
    /// An LLM message (user, assistant, tool result). The only variant sent to the LLM.
    Message(LlmMessage),

    /// Records a model switch during the session.
    ModelChange {
        from: ModelSpec,
        to: ModelSpec,
        timestamp: u64,
    },

    /// Records a thinking level change.
    ThinkingLevelChange {
        from: String,
        to: String,
        timestamp: u64,
    },

    /// Records a context compaction event.
    Compaction {
        dropped_count: usize,
        tokens_before: usize,
        tokens_after: usize,
        timestamp: u64,
    },

    /// A user bookmark/annotation on a specific message.
    Label {
        text: String,
        message_index: usize,
        timestamp: u64,
    },

    /// Arbitrary structured data for extensibility.
    Custom {
        type_name: String,
        data: serde_json::Value,
        timestamp: u64,
    },
}

impl SessionEntry {
    /// Parse a JSONL line into a `SessionEntry`.
    ///
    /// If the line has an `entry_type` field, it is deserialized as a tagged enum.
    /// Otherwise, it is treated as a raw `LlmMessage` (backward compatibility).
    pub fn parse(line: &str) -> Result<Self, serde_json::Error> {
        // Try tagged format first
        let value: serde_json::Value = serde_json::from_str(line)?;
        if value.get("entry_type").is_some() {
            return serde_json::from_value(value);
        }
        // Fallback: old-format raw LlmMessage
        serde_json::from_value::<LlmMessage>(value).map(SessionEntry::Message)
    }

    /// Returns the contained `LlmMessage` if this is a `Message` variant.
    pub const fn as_message(&self) -> Option<&LlmMessage> {
        match self {
            Self::Message(msg) => Some(msg),
            _ => None,
        }
    }

    /// Extract only `Message` entries from a slice, returning the `LlmMessage` values.
    pub fn messages(entries: &[Self]) -> Vec<&LlmMessage> {
        entries.iter().filter_map(Self::as_message).collect()
    }

    /// Returns the serde discriminator string for this entry variant.
    ///
    /// Matches the `rename_all = "snake_case"` representation used in serialization.
    pub const fn entry_type_name(&self) -> &'static str {
        match self {
            Self::Message(_) => "message",
            Self::ModelChange { .. } => "model_change",
            Self::ThinkingLevelChange { .. } => "thinking_level_change",
            Self::Compaction { .. } => "compaction",
            Self::Label { .. } => "label",
            Self::Custom { .. } => "custom",
        }
    }

    /// Returns the timestamp of this entry, if it has one.
    ///
    /// `Message` entries derive their timestamp from the inner `LlmMessage`.
    pub const fn timestamp(&self) -> Option<u64> {
        match self {
            // `LlmMessage` is `#[non_exhaustive]`: a variant this build
            // doesn't recognise carries no known timestamp field, so report
            // `None` rather than guessing at one.
            Self::Message(msg) => match msg {
                LlmMessage::User(m) => Some(m.timestamp),
                LlmMessage::Assistant(m) => Some(m.timestamp),
                LlmMessage::ToolResult(m) => Some(m.timestamp),
                _ => None,
            },
            Self::ModelChange { timestamp, .. }
            | Self::ThinkingLevelChange { timestamp, .. }
            | Self::Compaction { timestamp, .. }
            | Self::Label { timestamp, .. }
            | Self::Custom { timestamp, .. } => Some(*timestamp),
        }
    }
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SessionEntry>();
};

#[cfg(test)]
#[path = "entry_tests.rs"]
mod tests;
