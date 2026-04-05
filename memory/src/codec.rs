//! Canonical [`AgentMessage`] encode / decode used by all storage backends.
//!
//! Both the `JSONL` and `SQLite` backends previously duplicated the logic for
//! serializing / deserializing [`AgentMessage`] values (calling
//! [`serialize_custom_message`] / [`deserialize_custom_message`] inline,
//! each with its own warn-and-skip pattern). This module centralises that
//! shared concern so backends only deal with their own storage format.

use std::io;

use swink_agent::{
    AgentMessage, CustomMessageRegistry, LlmMessage, restore_single_custom,
    serialize_custom_message,
};

/// Discriminator tag for an encoded agent message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    /// An [`LlmMessage`] (user, assistant, tool result).
    Llm,
    /// A custom message serialised via [`CustomMessage::to_json`].
    Custom,
}

impl MessageKind {
    /// The string tag stored in the `SQLite` `kind` column.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Llm => "llm",
            Self::Custom => "custom",
        }
    }

    /// Parse a stored tag back to a [`MessageKind`]. Returns `None` for
    /// unknown tags so callers can skip unrecognised rows gracefully.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "llm" => Some(Self::Llm),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }
}

/// Encode an [`AgentMessage`] to a `(kind, json)` pair.
///
/// Returns `None` and emits a `tracing::warn` for custom messages that cannot
/// be serialised (i.e. their [`CustomMessage::to_json`] / [`CustomMessage::type_name`]
/// implementations return `None`). LLM messages always succeed unless
/// `serde_json` itself errors, which should not happen in practice.
pub fn encode(msg: &AgentMessage, session_id: &str) -> Option<(MessageKind, String)> {
    match msg {
        AgentMessage::Llm(llm) => serde_json::to_string(llm)
            .ok()
            .map(|json| (MessageKind::Llm, json)),
        AgentMessage::Custom(custom) => {
            let Some(envelope) = serialize_custom_message(custom.as_ref()) else {
                tracing::warn!(
                    session_id,
                    "skipping non-serializable CustomMessage: {custom:?}"
                );
                return None;
            };
            serde_json::to_string(&envelope)
                .ok()
                .map(|json| (MessageKind::Custom, json))
        }
    }
}

/// Decode a `(kind, json)` pair back to an [`AgentMessage`].
///
/// - `Llm`: deserialises the JSON directly; returns `Err` on malformed data.
/// - `Custom`: returns `Ok(None)` when no `registry` is provided (the caller
///   chose not to restore custom messages). Returns `Ok(None)` on a
///   deserialisation error so callers can skip without propagating.
pub fn decode(
    kind: MessageKind,
    data: &str,
    registry: Option<&CustomMessageRegistry>,
) -> io::Result<Option<AgentMessage>> {
    match kind {
        MessageKind::Llm => serde_json::from_str::<LlmMessage>(data)
            .map(|m| Some(AgentMessage::Llm(m)))
            .map_err(io::Error::other),
        MessageKind::Custom => {
            let envelope: serde_json::Value =
                serde_json::from_str(data).map_err(io::Error::other)?;
            restore_single_custom(registry, &envelope)
                .map(|opt| opt.map(AgentMessage::Custom))
                .map_err(io::Error::other)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swink_agent::{ContentBlock, LlmMessage, UserMessage};

    fn user_msg() -> AgentMessage {
        AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "hello".to_string(),
            }],
            timestamp: 1,
            cache_hint: None,
        }))
    }

    #[test]
    fn encode_decode_llm_roundtrip() {
        let msg = user_msg();
        let (kind, json) = encode(&msg, "test-session").expect("llm encodes");
        assert_eq!(kind, MessageKind::Llm);

        let decoded = decode(MessageKind::Llm, &json, None)
            .expect("no io error")
            .expect("decoded to Some");
        assert!(matches!(decoded, AgentMessage::Llm(LlmMessage::User(_))));
    }

    #[test]
    fn message_kind_parse_and_as_str() {
        assert_eq!(MessageKind::parse("llm"), Some(MessageKind::Llm));
        assert_eq!(MessageKind::parse("custom"), Some(MessageKind::Custom));
        assert_eq!(MessageKind::parse("unknown"), None);
        assert_eq!(MessageKind::Llm.as_str(), "llm");
        assert_eq!(MessageKind::Custom.as_str(), "custom");
    }

    #[test]
    fn decode_llm_errors_on_malformed_json() {
        let result = decode(MessageKind::Llm, "not-valid-json", None);
        assert!(result.is_err());
    }

    #[test]
    fn decode_custom_returns_none_without_registry() {
        // A minimal valid custom-message envelope.
        let envelope = serde_json::json!({
            "type_name": "MyType",
            "data": {}
        });
        let json = serde_json::to_string(&envelope).unwrap();
        let result = decode(MessageKind::Custom, &json, None).expect("no io error");
        assert!(result.is_none(), "no registry → None, not an error");
    }
}
