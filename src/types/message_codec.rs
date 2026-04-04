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
//! - [`SerializedCustomMessage`] — a lightweight [`CustomMessage`]
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
#[derive(Debug, Clone)]
pub struct SerializedMessages {
    /// LLM messages in insertion order.
    pub llm_messages: Vec<LlmMessage>,
    /// Custom message envelopes (`{"type": "…", "data": {…}}`).
    pub custom_messages: Vec<serde_json::Value>,
    /// Records the original interleaved order of LLM and custom messages.
    pub message_order: Vec<MessageSlot>,
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
                        "skipping non-serializable CustomMessage in {kind}: {:?}",
                        custom
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
    let mut result: Vec<AgentMessage> =
        llm_messages.iter().cloned().map(AgentMessage::Llm).collect();

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

/// A lightweight [`CustomMessage`] stand-in that holds pre-serialized data.
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
mod tests {
    use super::*;
    use crate::types::{
        AssistantMessage, ContentBlock, Cost, CustomMessage, StopReason, Usage, UserMessage,
    };

    // ── Test helpers ────────────────────────────────────────────────────────

    #[derive(Debug)]
    struct NonSerializableCustom;

    impl CustomMessage for NonSerializableCustom {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[derive(Debug, Clone, PartialEq)]
    struct TaggedCustom {
        tag: String,
    }

    impl CustomMessage for TaggedCustom {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn type_name(&self) -> Option<&str> {
            Some("TaggedCustom")
        }
        fn to_json(&self) -> Option<serde_json::Value> {
            Some(serde_json::json!({ "tag": self.tag }))
        }
    }

    fn tagged_registry() -> CustomMessageRegistry {
        let mut reg = CustomMessageRegistry::new();
        reg.register(
            "TaggedCustom",
            Box::new(|val: serde_json::Value| {
                let tag = val
                    .get("tag")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing tag".to_string())?;
                Ok(Box::new(TaggedCustom {
                    tag: tag.to_string(),
                }) as Box<dyn CustomMessage>)
            }),
        );
        reg
    }

    fn user_msg(text: &str) -> AgentMessage {
        AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            timestamp: 0,
            cache_hint: None,
        }))
    }

    fn assistant_msg(text: &str) -> AgentMessage {
        AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            provider: "test".to_string(),
            model_id: "m".to_string(),
            usage: Usage::default(),
            cost: Cost::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            error_kind: None,
            timestamp: 0,
            cache_hint: None,
        }))
    }

    fn custom_msg(tag: &str) -> AgentMessage {
        AgentMessage::Custom(Box::new(TaggedCustom {
            tag: tag.to_string(),
        }))
    }

    fn message_label(msg: &AgentMessage) -> String {
        match msg {
            AgentMessage::Llm(LlmMessage::User(u)) => {
                format!("user:{}", ContentBlock::extract_text(&u.content))
            }
            AgentMessage::Llm(LlmMessage::Assistant(a)) => {
                format!("assistant:{}", ContentBlock::extract_text(&a.content))
            }
            AgentMessage::Custom(c) => {
                if let Some(json) = c.to_json() {
                    format!("custom:{}", json["tag"].as_str().unwrap_or("?"))
                } else {
                    "custom:?".to_string()
                }
            }
            _ => "other".to_string(),
        }
    }

    // ── serialize_messages ──────────────────────────────────────────────────

    #[test]
    fn serialize_skips_non_serializable_custom() {
        let messages = vec![
            user_msg("hi"),
            AgentMessage::Custom(Box::new(NonSerializableCustom)),
            assistant_msg("hello"),
        ];

        let result = serialize_messages(&messages, "test");
        assert_eq!(result.llm_messages.len(), 2);
        assert!(result.custom_messages.is_empty());
        assert_eq!(result.message_order.len(), 2);
    }

    #[test]
    fn serialize_preserves_interleaved_order() {
        let messages = vec![
            user_msg("hello"),
            custom_msg("A"),
            assistant_msg("hi"),
            custom_msg("B"),
            user_msg("thanks"),
        ];

        let result = serialize_messages(&messages, "test");
        assert_eq!(result.llm_messages.len(), 3);
        assert_eq!(result.custom_messages.len(), 2);
        assert_eq!(result.message_order.len(), 5);

        // Verify envelope content
        assert_eq!(result.custom_messages[0]["type"], "TaggedCustom");
        assert_eq!(result.custom_messages[0]["data"]["tag"], "A");
        assert_eq!(result.custom_messages[1]["data"]["tag"], "B");
    }

    // ── restore_messages ───────────────────────────────────────────────────

    #[test]
    fn roundtrip_preserves_order() {
        let registry = tagged_registry();
        let messages = vec![
            user_msg("hello"),
            custom_msg("A"),
            assistant_msg("hi"),
            custom_msg("B"),
            user_msg("thanks"),
        ];

        let serialized = serialize_messages(&messages, "test");
        let restored = restore_messages(
            &serialized.llm_messages,
            &serialized.custom_messages,
            &serialized.message_order,
            Some(&registry),
            "test",
        );

        let labels: Vec<String> = restored.iter().map(message_label).collect();
        assert_eq!(
            labels,
            vec![
                "user:hello",
                "custom:A",
                "assistant:hi",
                "custom:B",
                "user:thanks",
            ]
        );
    }

    #[test]
    fn restore_without_registry_skips_custom() {
        let messages = vec![user_msg("hi"), custom_msg("skipped"), assistant_msg("ok")];

        let serialized = serialize_messages(&messages, "test");
        let restored = restore_messages(
            &serialized.llm_messages,
            &serialized.custom_messages,
            &serialized.message_order,
            None,
            "test",
        );

        assert_eq!(restored.len(), 2);
        let labels: Vec<String> = restored.iter().map(message_label).collect();
        assert_eq!(labels, vec!["user:hi", "assistant:ok"]);
    }

    #[test]
    fn legacy_fallback_no_ordering() {
        let registry = tagged_registry();
        let llm = vec![LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "hi".to_string(),
            }],
            timestamp: 0,
            cache_hint: None,
        })];
        let custom = vec![serde_json::json!({
            "type": "TaggedCustom",
            "data": { "tag": "legacy" }
        })];

        let restored = restore_messages(&llm, &custom, &[], Some(&registry), "test");
        assert_eq!(restored.len(), 2);
        let labels: Vec<String> = restored.iter().map(message_label).collect();
        assert_eq!(labels, vec!["user:hi", "custom:legacy"]);
    }

    // ── restore_single_custom ──────────────────────────────────────────────

    #[test]
    fn restore_single_custom_with_registry() {
        let registry = tagged_registry();
        let envelope = serde_json::json!({
            "type": "TaggedCustom",
            "data": { "tag": "single" }
        });

        let result = restore_single_custom(Some(&registry), &envelope).unwrap();
        assert!(result.is_some());
        let custom = result.unwrap();
        assert_eq!(custom.type_name(), Some("TaggedCustom"));
    }

    #[test]
    fn restore_single_custom_without_registry() {
        let envelope = serde_json::json!({ "type": "X", "data": {} });
        let result = restore_single_custom(None, &envelope).unwrap();
        assert!(result.is_none());
    }

    // ── SerializedCustomMessage ────────────────────────────────────────────

    #[test]
    fn serialized_custom_message_from_custom() {
        let original = TaggedCustom {
            tag: "hello".to_string(),
        };
        let snapshot = SerializedCustomMessage::from_custom(&original).unwrap();
        assert_eq!(snapshot.type_name(), Some("TaggedCustom"));
        assert_eq!(snapshot.to_json().unwrap()["tag"], "hello");
    }

    #[test]
    fn serialized_custom_message_from_non_serializable() {
        let bare = NonSerializableCustom;
        assert!(SerializedCustomMessage::from_custom(&bare).is_none());
    }

    // ── clone_messages_for_send ────────────────────────────────────────────

    #[test]
    fn clone_for_send_preserves_all_serializable() {
        let messages = vec![
            user_msg("hello"),
            custom_msg("kept"),
            AgentMessage::Custom(Box::new(NonSerializableCustom)),
            assistant_msg("world"),
        ];

        let cloned = clone_messages_for_send(&messages);
        assert_eq!(cloned.len(), 3); // non-serializable custom dropped
        let labels: Vec<String> = cloned.iter().map(message_label).collect();
        assert_eq!(labels, vec!["user:hello", "custom:kept", "assistant:world"]);
    }

    #[test]
    fn clone_for_send_custom_roundtrips_through_registry() {
        let registry = tagged_registry();
        let messages = vec![custom_msg("roundtrip")];
        let cloned = clone_messages_for_send(&messages);
        assert_eq!(cloned.len(), 1);

        // The cloned custom message can be serialized and restored
        let envelope = serialize_custom_message(
            cloned[0]
                .downcast_ref::<SerializedCustomMessage>()
                .unwrap(),
        )
        .unwrap();
        let restored = deserialize_custom_message(&registry, &envelope).unwrap();
        assert_eq!(
            restored
                .as_any()
                .downcast_ref::<TaggedCustom>()
                .unwrap()
                .tag,
            "roundtrip"
        );
    }
}
