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
}

#[cfg(test)]
mod tests {
    use super::*;
    use swink_agent::{ContentBlock, ModelSpec, UserMessage};

    #[test]
    fn session_entry_serde_roundtrip_message() {
        let entry = SessionEntry::Message(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "hello".to_string(),
            }],
            timestamp: 42,
            cache_hint: None,
        }));

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: SessionEntry = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, SessionEntry::Message(LlmMessage::User(_))));
    }

    #[test]
    fn session_entry_serde_roundtrip_model_change() {
        let entry = SessionEntry::ModelChange {
            from: ModelSpec {
                provider: "openai".to_string(),
                model_id: "gpt-4".to_string(),
                ..ModelSpec::new("", "")
            },
            to: ModelSpec {
                provider: "anthropic".to_string(),
                model_id: "claude-3".to_string(),
                ..ModelSpec::new("", "")
            },
            timestamp: 100,
        };

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: SessionEntry = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            parsed,
            SessionEntry::ModelChange { timestamp: 100, .. }
        ));
    }

    #[test]
    fn session_entry_serde_roundtrip_compaction() {
        let entry = SessionEntry::Compaction {
            dropped_count: 15,
            tokens_before: 5000,
            tokens_after: 2000,
            timestamp: 200,
        };

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: SessionEntry = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            parsed,
            SessionEntry::Compaction {
                dropped_count: 15,
                tokens_before: 5000,
                tokens_after: 2000,
                timestamp: 200,
            }
        ));
    }

    #[test]
    fn session_entry_serde_roundtrip_label() {
        let entry = SessionEntry::Label {
            text: "important".to_string(),
            message_index: 5,
            timestamp: 300,
        };

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: SessionEntry = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            parsed,
            SessionEntry::Label {
                message_index: 5,
                timestamp: 300,
                ..
            }
        ));
    }

    #[test]
    fn session_entry_serde_roundtrip_custom() {
        let entry = SessionEntry::Custom {
            type_name: "my_event".to_string(),
            data: serde_json::json!({"key": "value"}),
            timestamp: 400,
        };

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: SessionEntry = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            parsed,
            SessionEntry::Custom { timestamp: 400, .. }
        ));
    }

    #[test]
    fn session_entry_serde_roundtrip_thinking_level_change() {
        let entry = SessionEntry::ThinkingLevelChange {
            from: "low".to_string(),
            to: "high".to_string(),
            timestamp: 500,
        };

        let json = serde_json::to_string(&entry).unwrap();
        let parsed: SessionEntry = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            parsed,
            SessionEntry::ThinkingLevelChange { timestamp: 500, .. }
        ));
    }

    #[test]
    fn parse_old_format_as_message() {
        // Old-format: raw LlmMessage without entry_type
        let old_line = serde_json::to_string(&LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "old format".to_string(),
            }],
            timestamp: 0,
            cache_hint: None,
        }))
        .unwrap();

        let entry = SessionEntry::parse(&old_line).unwrap();
        assert!(matches!(entry, SessionEntry::Message(LlmMessage::User(_))));
    }

    #[test]
    fn parse_tagged_format() {
        let tagged = r#"{"entry_type":"label","data":{"text":"bookmark","message_index":3,"timestamp":100}}"#;
        let entry = SessionEntry::parse(tagged).unwrap();
        assert!(matches!(entry, SessionEntry::Label { .. }));
    }

    #[test]
    fn rich_entries_excluded_from_llm_context() {
        let entries = vec![
            SessionEntry::Message(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "hello".to_string(),
                }],
                timestamp: 0,
                cache_hint: None,
            })),
            SessionEntry::ModelChange {
                from: ModelSpec::new("test", "test"),
                to: ModelSpec::new("test", "test"),
                timestamp: 1,
            },
            SessionEntry::Label {
                text: "important".to_string(),
                message_index: 0,
                timestamp: 2,
            },
            SessionEntry::Compaction {
                dropped_count: 5,
                tokens_before: 1000,
                tokens_after: 500,
                timestamp: 3,
            },
            SessionEntry::Custom {
                type_name: "test".to_string(),
                data: serde_json::json!({}),
                timestamp: 4,
            },
            SessionEntry::Message(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "world".to_string(),
                }],
                timestamp: 5,
                cache_hint: None,
            })),
        ];

        let messages = SessionEntry::messages(&entries);
        assert_eq!(messages.len(), 2);
        // Only Message variants returned
        for msg in &messages {
            assert!(matches!(msg, LlmMessage::User(_)));
        }
    }
}
