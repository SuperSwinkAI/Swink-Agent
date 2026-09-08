//! Canonical persistence codecs used by all storage backends.
//!
//! This module centralises the shared encode / decode rules for:
//! - plain [`AgentMessage`] persistence
//! - JSONL message lines, including legacy `_custom` and `_state` records
//! - rich [`SessionEntry`] persistence used by filtered/history-aware loading
//!
//! Backends should only deal with their transport shape (line-based JSONL,
//! `(kind, data)` rows in `SQLite`, etc.), not reimplement message codecs.

use std::io;

use swink_agent::{
    AgentMessage, CustomMessageRegistry, LlmMessage, restore_single_custom,
    serialize_custom_message,
};

use crate::entry::SessionEntry;

/// Discriminator tag for an encoded agent message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageKind {
    /// An [`LlmMessage`] (user, assistant, tool result).
    Llm,
    /// A custom message serialised via `swink_agent::CustomMessage::to_json`.
    Custom,
}

impl MessageKind {
    /// The string tag stored in the `SQLite` `kind` column.
    #[allow(dead_code)]
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
/// be serialised (i.e. their `swink_agent::CustomMessage::to_json` /
/// `swink_agent::CustomMessage::type_name`
/// implementations return `None`). LLM messages always succeed unless
/// `serde_json` itself errors, which should not happen in practice.
#[allow(dead_code)]
fn encode(msg: &AgentMessage, session_id: &str) -> Option<(MessageKind, String)> {
    match msg {
        AgentMessage::Llm(llm) => serde_json::to_string(llm)
            .ok()
            .map(|json| (MessageKind::Llm, json)),
        AgentMessage::Custom(custom) => {
            let Some(envelope) = serialize_custom_message(custom.as_ref()) else {
                tracing::warn!(
                    session_id,
                    type_name = custom.type_name().unwrap_or("<unknown>"),
                    "skipping non-serializable CustomMessage"
                );
                return None;
            };
            serde_json::to_string(&envelope)
                .ok()
                .map(|json| (MessageKind::Custom, json))
        }
        // `AgentMessage` is `#[non_exhaustive]`: a variant this build
        // doesn't know how to encode can't be mapped to an existing
        // `MessageKind`, so skip it the same way an unserializable custom
        // message is skipped above.
        _ => {
            tracing::warn!(
                session_id,
                "skipping AgentMessage variant not recognised by this build"
            );
            None
        }
    }
}

/// Decode a `(kind, json)` pair back to an [`AgentMessage`].
///
/// - `Llm`: deserialises the JSON directly; returns `Err` on malformed data.
/// - `Custom`: returns `Ok(None)` when no `registry` is provided (the caller
///   chose not to restore custom messages). Returns `Ok(None)` on a
///   deserialisation error so callers can skip without propagating.
fn decode(
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

/// Encode an [`AgentMessage`] as a JSONL line.
///
/// LLM messages are stored as raw `LlmMessage` JSON for backward
/// compatibility. Custom messages are stored as their serialized envelope with
/// a `_custom: true` marker so `JSONL` can distinguish them from plain JSON
/// objects.
pub fn encode_jsonl_message_line(msg: &AgentMessage, session_id: &str) -> Option<String> {
    match msg {
        AgentMessage::Llm(llm) => serde_json::to_string(llm).ok(),
        AgentMessage::Custom(custom) => {
            let Some(envelope) = serialize_custom_message(custom.as_ref()) else {
                tracing::warn!(
                    session_id,
                    type_name = custom.type_name().unwrap_or("<unknown>"),
                    "skipping non-serializable CustomMessage"
                );
                return None;
            };

            let mut envelope = envelope;
            envelope
                .as_object_mut()
                .expect("custom message envelope must be an object")
                .insert("_custom".to_string(), serde_json::Value::Bool(true));

            serde_json::to_string(&envelope).ok()
        }
        // Same rationale as `encode` above: an unrecognised `AgentMessage`
        // variant has no JSONL encoding this build knows how to produce.
        _ => {
            tracing::warn!(
                session_id,
                "skipping AgentMessage variant not recognised by this build"
            );
            None
        }
    }
}

/// Decode one JSONL line back into an [`AgentMessage`].
///
/// Returns `Ok(None)` for state records, for entry-tagged records that are not
/// messages, and when a custom message cannot be restored because no registry
/// was supplied.
pub fn decode_jsonl_message_line(
    line: &str,
    registry: Option<&CustomMessageRegistry>,
) -> io::Result<Option<AgentMessage>> {
    let value: serde_json::Value = serde_json::from_str(line).map_err(io::Error::other)?;
    if value.get("_state").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(None);
    }

    if value.get("_custom").and_then(serde_json::Value::as_bool) == Some(true) {
        return decode(MessageKind::Custom, line, registry);
    }

    if let Ok(message) = serde_json::from_value::<LlmMessage>(value) {
        return Ok(Some(AgentMessage::Llm(message)));
    }

    match SessionEntry::parse(line).map_err(io::Error::other)? {
        SessionEntry::Message(message) => Ok(Some(AgentMessage::Llm(message))),
        _ => Ok(None),
    }
}

/// Encode a rich [`SessionEntry`] for row-based backends such as `SQLite`.
///
/// The returned `kind` is a lightweight discriminator column, while `data`
/// contains the canonical JSON payload. Message entries keep their
/// `entry_type = "message"` tagged representation for compatibility with
/// existing rich-entry storage.
#[allow(dead_code)]
fn encode_session_entry(entry: &SessionEntry) -> io::Result<(String, String)> {
    let kind = entry.entry_type_name().to_string();
    let data = serde_json::to_string(entry).map_err(io::Error::other)?;
    Ok((kind, data))
}

/// Decode a rich [`SessionEntry`] from a row-based backend.
///
/// Returns `Ok(None)` for stored custom-agent-message rows (`kind = "custom"`),
/// which are not representable as `SessionEntry`.
#[allow(dead_code)]
fn decode_session_entry(kind: &str, data: &str) -> io::Result<Option<SessionEntry>> {
    match MessageKind::parse(kind) {
        Some(MessageKind::Llm) => serde_json::from_str::<LlmMessage>(data)
            .map(SessionEntry::Message)
            .map(Some)
            .map_err(io::Error::other),
        Some(MessageKind::Custom) => serde_json::from_str::<SessionEntry>(data)
            .map_or_else(|_| Ok(None), |entry| Ok(Some(entry))),
        None => serde_json::from_str::<SessionEntry>(data)
            .map(Some)
            .map_err(io::Error::other),
    }
}

#[cfg(test)]
#[path = "codec_tests.rs"]
mod tests;
