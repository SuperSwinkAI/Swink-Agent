use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use super::LlmMessage;

// ─── Custom Messages ────────────────────────────────────────────────────────

/// Trait for application-defined custom message types.
///
/// Allows downstream code to attach application-specific message types
/// (e.g. notifications, artifacts) to the message history without modifying
/// the harness.
///
/// ## Serialization
///
/// To support store/load of conversations containing custom messages, implement
/// [`type_name`](Self::type_name) and [`to_json`](Self::to_json), then register
/// a deserializer with [`CustomMessageRegistry`].
pub trait CustomMessage: Send + Sync + fmt::Debug + std::any::Any {
    /// Downcast helper. Returns `self` as `&dyn Any` for type-safe downcasting.
    fn as_any(&self) -> &dyn std::any::Any;

    /// A unique, stable identifier for this custom message type.
    ///
    /// Used as the discriminator when serializing. Must match the key
    /// registered in [`CustomMessageRegistry`]. Returns `None` if
    /// serialization is not supported.
    fn type_name(&self) -> Option<&str> {
        None
    }

    /// Serialize this custom message to a JSON value.
    ///
    /// Returns `None` if serialization is not supported (the default).
    fn to_json(&self) -> Option<serde_json::Value> {
        None
    }
}

/// A function that deserializes a JSON value into a boxed [`CustomMessage`].
pub type CustomMessageDeserializer =
    Box<dyn Fn(serde_json::Value) -> Result<Box<dyn CustomMessage>, String> + Send + Sync>;

/// Registry for deserializing [`CustomMessage`] types from JSON.
///
/// Each custom message type that supports serialization must register a
/// deserializer keyed by its [`CustomMessage::type_name`].
pub struct CustomMessageRegistry {
    deserializers: HashMap<String, CustomMessageDeserializer>,
}

impl CustomMessageRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            deserializers: HashMap::new(),
        }
    }

    /// Register a deserializer for a custom message type.
    ///
    /// The `type_name` must match the value returned by the corresponding
    /// [`CustomMessage::type_name`] implementation.
    pub fn register(
        &mut self,
        type_name: impl Into<String>,
        deserializer: CustomMessageDeserializer,
    ) {
        self.deserializers.insert(type_name.into(), deserializer);
    }

    /// Convenience method: register a type that implements `serde::Deserialize`.
    ///
    /// Equivalent to calling [`register`](Self::register) with a closure that
    /// deserializes via `serde_json::from_value`.
    pub fn register_type<T>(&mut self, type_name: impl Into<String>)
    where
        T: CustomMessage + serde::de::DeserializeOwned + 'static,
    {
        self.deserializers.insert(
            type_name.into(),
            Box::new(|value| {
                serde_json::from_value::<T>(value)
                    .map(|v| Box::new(v) as Box<dyn CustomMessage>)
                    .map_err(|e| e.to_string())
            }),
        );
    }

    /// Deserialize a custom message from its type name and JSON payload.
    ///
    /// # Errors
    ///
    /// Returns `Err` if no deserializer is registered for `type_name` or if
    /// deserialization fails.
    pub fn deserialize(
        &self,
        type_name: &str,
        value: serde_json::Value,
    ) -> Result<Box<dyn CustomMessage>, String> {
        let deser = self.deserializers.get(type_name).ok_or_else(|| {
            format!("no deserializer registered for custom message type: {type_name}")
        })?;
        deser(value)
    }

    /// Returns `true` if a deserializer is registered for `type_name`.
    #[must_use]
    pub fn has_type_name(&self, type_name: &str) -> bool {
        self.deserializers.contains_key(type_name)
    }
}

impl Default for CustomMessageRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for CustomMessageRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CustomMessageRegistry")
            .field(
                "registered_types",
                &self.deserializers.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Serialize a [`CustomMessage`] into a portable JSON envelope.
///
/// Returns `None` if the message does not support serialization (i.e.
/// `type_name()` or `to_json()` returns `None`).
#[must_use]
pub fn serialize_custom_message(msg: &dyn CustomMessage) -> Option<serde_json::Value> {
    let type_name = msg.type_name()?;
    let payload = msg.to_json()?;
    Some(serde_json::json!({
        "type": type_name,
        "data": payload,
    }))
}

/// Deserialize a [`CustomMessage`] from a JSON envelope produced by
/// [`serialize_custom_message`].
///
/// # Errors
///
/// Returns `Err` if the envelope is malformed, the type is unknown, or
/// deserialization fails.
pub fn deserialize_custom_message(
    registry: &CustomMessageRegistry,
    envelope: &serde_json::Value,
) -> Result<Box<dyn CustomMessage>, String> {
    let type_name = envelope
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing 'type' field in custom message envelope".to_string())?;
    let data = envelope
        .get("data")
        .cloned()
        .ok_or_else(|| "missing 'data' field in custom message envelope".to_string())?;
    registry.deserialize(type_name, data)
}

/// The top-level message type that wraps either an LLM message or a custom
/// application-defined message.
///
/// Implements [`Serialize`] so events containing messages can be forwarded
/// across process boundaries. `Llm` variants delegate to the derived impl;
/// `Custom` variants serialize via [`serialize_custom_message`] (or as
/// `null` if the custom message does not support serialization).
#[allow(clippy::large_enum_variant)]
pub enum AgentMessage {
    /// A standard LLM message (user, assistant, or tool result).
    Llm(LlmMessage),

    /// A custom application-defined message.
    Custom(Box<dyn CustomMessage>),
}

impl AgentMessage {
    /// Get the cache hint for this message (if any).
    ///
    /// Returns `None` for `Custom` messages (they are never sent to the LLM).
    pub const fn cache_hint(&self) -> Option<&crate::context_cache::CacheHint> {
        match self {
            Self::Llm(msg) => match msg {
                LlmMessage::User(m) => m.cache_hint.as_ref(),
                LlmMessage::Assistant(m) => m.cache_hint.as_ref(),
                LlmMessage::ToolResult(m) => m.cache_hint.as_ref(),
            },
            Self::Custom(_) => None,
        }
    }

    /// Set the cache hint on this message.
    ///
    /// No-op for `Custom` messages.
    pub const fn set_cache_hint(&mut self, hint: crate::context_cache::CacheHint) {
        match self {
            Self::Llm(msg) => match msg {
                LlmMessage::User(m) => m.cache_hint = Some(hint),
                LlmMessage::Assistant(m) => m.cache_hint = Some(hint),
                LlmMessage::ToolResult(m) => m.cache_hint = Some(hint),
            },
            Self::Custom(_) => {}
        }
    }

    /// Clear the cache hint on this message.
    pub const fn clear_cache_hint(&mut self) {
        match self {
            Self::Llm(msg) => match msg {
                LlmMessage::User(m) => m.cache_hint = None,
                LlmMessage::Assistant(m) => m.cache_hint = None,
                LlmMessage::ToolResult(m) => m.cache_hint = None,
            },
            Self::Custom(_) => {}
        }
    }

    /// Attempt to downcast the inner custom message to a concrete type.
    ///
    /// Returns `Ok(&T)` if this is a `Custom` variant and the inner type matches `T`.
    /// Returns `Err(DowncastError)` if this is an `Llm` variant or the type does not match.
    pub fn downcast_ref<T: 'static>(&self) -> Result<&T, crate::error::DowncastError> {
        match self {
            Self::Custom(msg) => {
                msg.as_any()
                    .downcast_ref::<T>()
                    .ok_or_else(|| crate::error::DowncastError {
                        expected: std::any::type_name::<T>(),
                        actual: msg
                            .type_name()
                            .map_or_else(|| format!("{msg:?}"), ToString::to_string),
                    })
            }
            Self::Llm(_) => Err(crate::error::DowncastError {
                expected: std::any::type_name::<T>(),
                actual: "LlmMessage".to_string(),
            }),
        }
    }
}

impl fmt::Debug for AgentMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Llm(msg) => f.debug_tuple("Llm").field(msg).finish(),
            Self::Custom(msg) => f.debug_tuple("Custom").field(msg).finish(),
        }
    }
}

impl Serialize for AgentMessage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Llm(msg) => {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("kind", "llm")?;
                map.serialize_entry("message", msg)?;
                map.end()
            }
            Self::Custom(msg) => {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("kind", "custom")?;
                // Use the existing envelope helper; falls back to null.
                let envelope = serialize_custom_message(msg.as_ref());
                map.serialize_entry("message", &envelope)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for AgentMessage {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Tagged {
            kind: String,
            message: serde_json::Value,
        }

        let tagged = Tagged::deserialize(deserializer)?;
        match tagged.kind.as_str() {
            "llm" => {
                let msg: LlmMessage =
                    serde_json::from_value(tagged.message).map_err(serde::de::Error::custom)?;
                Ok(Self::Llm(msg))
            }
            "custom" => Err(serde::de::Error::custom(
                "cannot deserialize AgentMessage::Custom (requires runtime type info)",
            )),
            other => Err(serde::de::Error::unknown_variant(
                other,
                &["llm", "custom"],
            )),
        }
    }
}
