//! Foundation types for the swink agent.
//!
//! This module defines every type that crosses a public boundary in the harness.
//! All other modules depend on it; it depends on nothing else in the crate.

use std::collections::HashMap;
use std::fmt;
use std::ops::{Add, AddAssign};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

// ─── Content ────────────────────────────────────────────────────────────────

/// The atomic unit of all message content.
///
/// Different variants are permitted in different message roles:
/// - `Text`: user, assistant, tool result
/// - `Thinking`: assistant only
/// - `ToolCall`: assistant only
/// - `Image`: user, tool result
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// A plain text string.
    Text { text: String },

    /// A reasoning / chain-of-thought string with an optional provider signature.
    Thinking {
        thinking: String,
        signature: Option<String>,
    },

    /// A tool invocation with an ID, tool name, parsed arguments, and an
    /// optional partial JSON buffer used during streaming.
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
        partial_json: Option<String>,
    },

    /// Image data from a supported source type.
    Image { source: ImageSource },

    /// An extension content block for plugin-defined types.
    ///
    /// Allows multimodal plugins to pass structured data without flattening to `Text`.
    Extension {
        type_name: String,
        data: serde_json::Value,
    },
}

impl ContentBlock {
    /// Extract concatenated text from a slice of content blocks.
    ///
    /// Returns the joined text of all `Text` variants, ignoring other block types.
    pub fn extract_text(blocks: &[Self]) -> String {
        let mut result = String::new();
        for block in blocks {
            if let Self::Text { text } = block {
                result.push_str(text);
            }
        }
        result
    }
}

/// Source for image data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    /// Base64-encoded image data with a media type.
    Base64 { media_type: String, data: String },

    /// A URL pointing to an image.
    Url { url: String },
}

// ─── Messages ───────────────────────────────────────────────────────────────

/// A message from the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMessage {
    pub content: Vec<ContentBlock>,
    pub timestamp: u64,
}

/// A message from the assistant (LLM response).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub content: Vec<ContentBlock>,
    pub provider: String,
    pub model_id: String,
    pub usage: Usage,
    pub cost: Cost,
    pub stop_reason: StopReason,
    pub error_message: Option<String>,
    pub timestamp: u64,
}

/// The result of a tool execution, sent back to the LLM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResultMessage {
    pub tool_call_id: String,
    pub content: Vec<ContentBlock>,
    pub is_error: bool,
    pub timestamp: u64,
    /// Structured data for display — not sent to the LLM.
    #[serde(default)]
    pub details: serde_json::Value,
}

/// A discriminated union of the three LLM message roles.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum LlmMessage {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
}

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
        let deser = self
            .deserializers
            .get(type_name)
            .ok_or_else(|| format!("no deserializer registered for custom message type: {type_name}"))?;
        deser(value)
    }

    /// Returns `true` if a deserializer is registered for `type_name`.
    #[must_use]
    pub fn contains(&self, type_name: &str) -> bool {
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
            .field("registered_types", &self.deserializers.keys().collect::<Vec<_>>())
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
#[allow(clippy::large_enum_variant)]
pub enum AgentMessage {
    /// A standard LLM message (user, assistant, or tool result).
    Llm(LlmMessage),

    /// A custom application-defined message.
    Custom(Box<dyn CustomMessage>),
}

impl fmt::Debug for AgentMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Llm(msg) => f.debug_tuple("Llm").field(msg).finish(),
            Self::Custom(msg) => f.debug_tuple("Custom").field(msg).finish(),
        }
    }
}

// ─── Usage & Cost ───────────────────────────────────────────────────────────

/// Token usage counters for an LLM response.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
    /// Provider-specific extra metrics (reasoning tokens, search tokens, etc.).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub extra: HashMap<String, u64>,
}

impl Usage {
    /// Merge another `Usage` into this one by summing all fields.
    pub fn merge(&mut self, other: &Self) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.total += other.total;
        for (k, v) in &other.extra {
            *self.extra.entry(k.clone()).or_insert(0) += v;
        }
    }
}

impl Add for Usage {
    type Output = Self;

    fn add(mut self, rhs: Self) -> Self::Output {
        self.input += rhs.input;
        self.output += rhs.output;
        self.cache_read += rhs.cache_read;
        self.cache_write += rhs.cache_write;
        self.total += rhs.total;
        for (k, v) in rhs.extra {
            *self.extra.entry(k).or_insert(0) += v;
        }
        self
    }
}

impl AddAssign for Usage {
    fn add_assign(&mut self, rhs: Self) {
        self.input += rhs.input;
        self.output += rhs.output;
        self.cache_read += rhs.cache_read;
        self.cache_write += rhs.cache_write;
        self.total += rhs.total;
        for (k, v) in rhs.extra {
            *self.extra.entry(k).or_insert(0) += v;
        }
    }
}

/// Per-category and total cost breakdown (floating-point currency values).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Cost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub total: f64,
    /// Provider-specific extra cost categories.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub extra: HashMap<String, f64>,
}

impl Add for Cost {
    type Output = Self;

    fn add(mut self, rhs: Self) -> Self::Output {
        self.input += rhs.input;
        self.output += rhs.output;
        self.cache_read += rhs.cache_read;
        self.cache_write += rhs.cache_write;
        self.total += rhs.total;
        for (k, v) in rhs.extra {
            *self.extra.entry(k).or_insert(0.0) += v;
        }
        self
    }
}

impl AddAssign for Cost {
    fn add_assign(&mut self, rhs: Self) {
        self.input += rhs.input;
        self.output += rhs.output;
        self.cache_read += rhs.cache_read;
        self.cache_write += rhs.cache_write;
        self.total += rhs.total;
        for (k, v) in rhs.extra {
            *self.extra.entry(k).or_insert(0.0) += v;
        }
    }
}

// ─── Stop Reason ────────────────────────────────────────────────────────────

/// Indicates why assistant generation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Natural end of generation.
    Stop,
    /// Output token limit reached.
    Length,
    /// Model requested a tool call.
    ToolUse,
    /// Cancelled by the caller.
    Aborted,
    /// An error occurred during generation.
    Error,
}

// ─── Model Capabilities ─────────────────────────────────────────────────────

/// Per-model capability flags and limits.
///
/// Populated from the model catalog or set manually. The agent loop can
/// inspect these before enabling provider-specific features (e.g. skip
/// thinking blocks when `supports_thinking` is false).
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ModelCapabilities {
    /// Whether the model supports extended-thinking / chain-of-thought blocks.
    pub supports_thinking: bool,
    /// Whether the model accepts image content blocks.
    pub supports_vision: bool,
    /// Whether the model can invoke tools.
    pub supports_tool_use: bool,
    /// Whether the model supports streaming responses.
    pub supports_streaming: bool,
    /// Whether the model supports structured (JSON schema) output.
    pub supports_structured_output: bool,
    /// Maximum input context window in tokens, if known.
    pub max_context_window: Option<u64>,
    /// Maximum output tokens per response, if known.
    pub max_output_tokens: Option<u64>,
}

impl ModelCapabilities {
    /// Create capabilities with all flags set to false and no limits.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Set `supports_thinking`.
    #[must_use]
    pub const fn with_thinking(mut self, val: bool) -> Self {
        self.supports_thinking = val;
        self
    }

    /// Set `supports_vision`.
    #[must_use]
    pub const fn with_vision(mut self, val: bool) -> Self {
        self.supports_vision = val;
        self
    }

    /// Set `supports_tool_use`.
    #[must_use]
    pub const fn with_tool_use(mut self, val: bool) -> Self {
        self.supports_tool_use = val;
        self
    }

    /// Set `supports_streaming`.
    #[must_use]
    pub const fn with_streaming(mut self, val: bool) -> Self {
        self.supports_streaming = val;
        self
    }

    /// Set `supports_structured_output`.
    #[must_use]
    pub const fn with_structured_output(mut self, val: bool) -> Self {
        self.supports_structured_output = val;
        self
    }

    /// Set `max_context_window`.
    #[must_use]
    pub const fn with_max_context_window(mut self, tokens: u64) -> Self {
        self.max_context_window = Some(tokens);
        self
    }

    /// Set `max_output_tokens`.
    #[must_use]
    pub const fn with_max_output_tokens(mut self, tokens: u64) -> Self {
        self.max_output_tokens = Some(tokens);
        self
    }
}

// ─── Model Specification ────────────────────────────────────────────────────

/// Reasoning depth for models that support configurable thinking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    #[default]
    Off,
    Minimal,
    Low,
    Medium,
    High,
    ExtraHigh,
}

/// Optional per-level token budget overrides for providers that support
/// token-based reasoning control.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingBudgets {
    pub budgets: HashMap<ThinkingLevel, u64>,
}

impl ThinkingBudgets {
    /// Create a new `ThinkingBudgets` from a map.
    #[must_use]
    pub const fn new(budgets: HashMap<ThinkingLevel, u64>) -> Self {
        Self { budgets }
    }

    /// Look up the token budget for a given thinking level.
    #[must_use]
    pub fn get(&self, level: &ThinkingLevel) -> Option<u64> {
        self.budgets.get(level).copied()
    }
}

/// Identifies the target model for a request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct ModelSpec {
    pub provider: String,
    pub model_id: String,
    pub thinking_level: ThinkingLevel,
    pub thinking_budgets: Option<ThinkingBudgets>,
    /// Provider-specific configuration (thinking, parameters, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_config: Option<serde_json::Value>,
    /// Per-model capability flags and limits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ModelCapabilities>,
}

impl ModelSpec {
    /// Create a new `ModelSpec` with thinking disabled and no budgets.
    #[must_use]
    pub fn new(provider: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model_id: model_id.into(),
            thinking_level: ThinkingLevel::Off,
            thinking_budgets: None,
            provider_config: None,
            capabilities: None,
        }
    }

    /// Set the reasoning depth for this model specification.
    #[must_use]
    pub const fn with_thinking_level(mut self, level: ThinkingLevel) -> Self {
        self.thinking_level = level;
        self
    }

    /// Set per-level token budget overrides for reasoning control.
    #[must_use]
    pub fn with_thinking_budgets(mut self, budgets: ThinkingBudgets) -> Self {
        self.thinking_budgets = Some(budgets);
        self
    }

    /// Set provider-specific configuration.
    #[must_use]
    pub fn with_provider_config(mut self, config: serde_json::Value) -> Self {
        self.provider_config = Some(config);
        self
    }

    /// Set model capabilities.
    #[must_use]
    pub const fn with_capabilities(mut self, capabilities: ModelCapabilities) -> Self {
        self.capabilities = Some(capabilities);
        self
    }

    /// Returns the model capabilities, or a default (all-false) set if none
    /// were provided.
    #[must_use]
    pub fn capabilities(&self) -> ModelCapabilities {
        self.capabilities.clone().unwrap_or_default()
    }

    /// Get a typed provider config, deserializing from the stored JSON.
    #[must_use]
    pub fn provider_config_as<T: serde::de::DeserializeOwned>(&self) -> Option<T> {
        self.provider_config
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }
}

// ─── Agent Result ───────────────────────────────────────────────────────────

/// The value returned by non-streaming invocations.
pub struct AgentResult {
    /// All new messages produced during the run.
    pub messages: Vec<AgentMessage>,
    /// The final stop reason from the last assistant message.
    pub stop_reason: StopReason,
    /// Aggregated token usage across all turns in the run.
    pub usage: Usage,
    /// Aggregated cost across all turns in the run.
    pub cost: Cost,
    /// Optional error string if the run ended in an error state.
    pub error: Option<String>,
}

impl fmt::Debug for AgentResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentResult")
            .field("messages", &self.messages)
            .field("stop_reason", &self.stop_reason)
            .field("usage", &self.usage)
            .field("cost", &self.cost)
            .field("error", &self.error)
            .finish()
    }
}

// ─── Agent Context ──────────────────────────────────────────────────────────

/// The immutable snapshot passed into each loop turn.
///
/// Contains the system prompt, current message history, and the list of
/// available tools. The loop never mutates a context in place — each turn
/// produces a new snapshot.
pub struct AgentContext {
    pub system_prompt: String,
    pub messages: Vec<AgentMessage>,
    /// The tools available during this turn.
    pub tools: Vec<Arc<dyn crate::tool::AgentTool>>,
}

impl fmt::Debug for AgentContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentContext")
            .field("system_prompt", &self.system_prompt)
            .field("messages", &self.messages)
            .field("tools", &format_args!("[{} tool(s)]", self.tools.len()))
            .finish()
    }
}

// ─── Serde Helpers ──────────────────────────────────────────────────────

fn serialize_arc_vec<S, T>(value: &Arc<Vec<T>>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
    T: Serialize,
{
    value.as_ref().serialize(serializer)
}

fn deserialize_arc_vec<'de, D, T>(deserializer: D) -> Result<Arc<Vec<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    let v = Vec::<T>::deserialize(deserializer)?;
    Ok(Arc::new(v))
}

// ─── Turn Snapshot ──────────────────────────────────────────────────────

/// A point-in-time snapshot of agent state at a turn boundary.
///
/// Emitted as part of `TurnEnd` events to support external replay, auditing,
/// and debugging. Contains the full context at the moment the turn completed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnSnapshot {
    /// Zero-based index of this turn within the current agent loop run.
    pub turn_index: usize,
    /// The LLM messages present in the context at the turn boundary.
    ///
    /// Wrapped in `Arc` to avoid cloning the full message list when the
    /// snapshot is forwarded to multiple subscribers.
    #[serde(
        serialize_with = "serialize_arc_vec",
        deserialize_with = "deserialize_arc_vec"
    )]
    pub messages: Arc<Vec<LlmMessage>>,
    /// Accumulated token usage up to and including this turn.
    pub usage: Usage,
    /// Accumulated cost up to and including this turn.
    pub cost: Cost,
    /// Stop reason from the assistant message that ended this turn.
    pub stop_reason: StopReason,
}

// ─── Compile-time Send + Sync assertions ────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<ContentBlock>();
    assert_send_sync::<ImageSource>();
    assert_send_sync::<UserMessage>();
    assert_send_sync::<AssistantMessage>();
    assert_send_sync::<ToolResultMessage>();
    assert_send_sync::<LlmMessage>();
    assert_send_sync::<AgentMessage>();
    assert_send_sync::<Usage>();
    assert_send_sync::<Cost>();
    assert_send_sync::<StopReason>();
    assert_send_sync::<ThinkingLevel>();
    assert_send_sync::<ThinkingBudgets>();
    assert_send_sync::<ModelCapabilities>();
    assert_send_sync::<ModelSpec>();
    assert_send_sync::<AgentResult>();
    assert_send_sync::<AgentContext>();
    assert_send_sync::<TurnSnapshot>();
    assert_send_sync::<CustomMessageRegistry>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_block_extension_serde_roundtrip() {
        let block = ContentBlock::Extension {
            type_name: "audio_clip".into(),
            data: serde_json::json!({"duration_ms": 1500, "codec": "opus"}),
        };
        let json = serde_json::to_string(&block).unwrap();
        let parsed: ContentBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(block, parsed);
    }

    #[test]
    fn extract_text_ignores_extension() {
        let blocks = vec![
            ContentBlock::Text {
                text: "hello ".into(),
            },
            ContentBlock::Extension {
                type_name: "image".into(),
                data: serde_json::json!({"url": "https://example.com/img.png"}),
            },
            ContentBlock::Text {
                text: "world".into(),
            },
        ];
        assert_eq!(ContentBlock::extract_text(&blocks), "hello world");
    }

    #[test]
    fn usage_extra_add_merges_maps() {
        let a = Usage {
            input: 10,
            output: 5,
            extra: HashMap::from([
                ("reasoning_tokens".into(), 100),
                ("search_tokens".into(), 50),
            ]),
            ..Default::default()
        };
        let b = Usage {
            input: 20,
            output: 10,
            extra: HashMap::from([("reasoning_tokens".into(), 200), ("new_metric".into(), 30)]),
            ..Default::default()
        };
        let c = a + b;
        assert_eq!(c.input, 30);
        assert_eq!(c.output, 15);
        assert_eq!(c.extra["reasoning_tokens"], 300);
        assert_eq!(c.extra["search_tokens"], 50);
        assert_eq!(c.extra["new_metric"], 30);
    }

    #[test]
    fn cost_extra_add_merges_maps() {
        let a = Cost {
            input: 0.01,
            output: 0.02,
            extra: HashMap::from([("reasoning_cost".into(), 0.05)]),
            ..Default::default()
        };
        let b = Cost {
            input: 0.03,
            output: 0.04,
            extra: HashMap::from([
                ("reasoning_cost".into(), 0.10),
                ("search_cost".into(), 0.02),
            ]),
            ..Default::default()
        };
        let c = a + b;
        assert!((c.input - 0.04).abs() < f64::EPSILON);
        assert!((c.output - 0.06).abs() < f64::EPSILON);
        assert!((c.extra["reasoning_cost"] - 0.15).abs() < f64::EPSILON);
        assert!((c.extra["search_cost"] - 0.02).abs() < f64::EPSILON);
    }

    #[test]
    fn model_spec_with_provider_config() {
        let config = serde_json::json!({
            "temperature": 0.7,
            "top_p": 0.9,
        });

        let spec = ModelSpec::new("anthropic", "claude-3").with_provider_config(config.clone());

        assert_eq!(spec.provider_config, Some(config));
        assert_eq!(spec.provider, "anthropic");
        assert_eq!(spec.model_id, "claude-3");
    }

    #[test]
    fn provider_config_as_typed() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct MyConfig {
            temperature: f64,
            max_tokens: u32,
        }

        let spec = ModelSpec::new("openai", "gpt-4").with_provider_config(serde_json::json!({
            "temperature": 0.5,
            "max_tokens": 1024,
        }));

        let config: Option<MyConfig> = spec.provider_config_as();
        assert_eq!(
            config,
            Some(MyConfig {
                temperature: 0.5,
                max_tokens: 1024,
            })
        );

        // None when no provider_config is set.
        let spec_none = ModelSpec::new("openai", "gpt-4");
        let config_none: Option<MyConfig> = spec_none.provider_config_as();
        assert!(config_none.is_none());
    }

    #[test]
    fn model_capabilities_builder_chain() {
        let caps = ModelCapabilities::none()
            .with_thinking(true)
            .with_vision(true)
            .with_tool_use(true)
            .with_streaming(true)
            .with_structured_output(true)
            .with_max_context_window(200_000)
            .with_max_output_tokens(16384);

        assert!(caps.supports_thinking);
        assert!(caps.supports_vision);
        assert!(caps.supports_tool_use);
        assert!(caps.supports_streaming);
        assert!(caps.supports_structured_output);
        assert_eq!(caps.max_context_window, Some(200_000));
        assert_eq!(caps.max_output_tokens, Some(16384));
    }

    #[test]
    fn model_capabilities_serde_roundtrip() {
        let caps = ModelCapabilities::none()
            .with_thinking(true)
            .with_tool_use(true)
            .with_max_context_window(128_000);
        let json = serde_json::to_string(&caps).unwrap();
        let parsed: ModelCapabilities = serde_json::from_str(&json).unwrap();
        assert_eq!(caps, parsed);
    }

    #[test]
    fn model_spec_with_capabilities() {
        let caps = ModelCapabilities::none()
            .with_thinking(true)
            .with_streaming(true);
        let spec = ModelSpec::new("test", "model-1").with_capabilities(caps.clone());
        assert_eq!(spec.capabilities, Some(caps.clone()));
        assert_eq!(spec.capabilities(), caps);
    }

    #[test]
    fn model_spec_capabilities_defaults_when_none() {
        let spec = ModelSpec::new("test", "model-1");
        assert!(spec.capabilities.is_none());
        let caps = spec.capabilities();
        assert!(!caps.supports_thinking);
        assert_eq!(caps.max_context_window, None);
    }

    // ─── Custom Message Serialization ────────────────────────────────────

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct MockNotification {
        title: String,
        body: String,
    }

    impl CustomMessage for MockNotification {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn type_name(&self) -> Option<&str> {
            Some("mock_notification")
        }

        fn to_json(&self) -> Option<serde_json::Value> {
            serde_json::to_value(self).ok()
        }
    }

    #[test]
    fn custom_message_serialize_roundtrip() {
        let msg = MockNotification {
            title: "Hello".into(),
            body: "World".into(),
        };

        let envelope = serialize_custom_message(&msg).expect("serialization supported");
        assert_eq!(envelope["type"], "mock_notification");
        assert_eq!(envelope["data"]["title"], "Hello");

        let mut registry = CustomMessageRegistry::new();
        registry.register_type::<MockNotification>("mock_notification");

        let restored = deserialize_custom_message(&registry, &envelope).unwrap();
        let downcasted = restored.as_any().downcast_ref::<MockNotification>().unwrap();
        assert_eq!(downcasted, &msg);
    }

    #[test]
    fn custom_message_default_returns_none() {
        #[derive(Debug)]
        struct Bare;
        impl CustomMessage for Bare {
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
        }
        let bare = Bare;
        assert!(bare.type_name().is_none());
        assert!(bare.to_json().is_none());
        assert!(serialize_custom_message(&bare).is_none());
    }

    #[test]
    fn registry_unknown_type_returns_error() {
        let registry = CustomMessageRegistry::new();
        let envelope = serde_json::json!({"type": "unknown", "data": {}});
        let result = deserialize_custom_message(&registry, &envelope);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no deserializer registered"));
    }

    #[test]
    fn registry_contains_check() {
        let mut registry = CustomMessageRegistry::new();
        assert!(!registry.contains("mock_notification"));
        registry.register_type::<MockNotification>("mock_notification");
        assert!(registry.contains("mock_notification"));
    }

    #[test]
    fn deserialize_custom_message_missing_fields() {
        let registry = CustomMessageRegistry::new();

        let no_type = serde_json::json!({"data": {}});
        assert!(deserialize_custom_message(&registry, &no_type)
            .unwrap_err()
            .contains("missing 'type'"));

        let no_data = serde_json::json!({"type": "foo"});
        assert!(deserialize_custom_message(&registry, &no_data)
            .unwrap_err()
            .contains("missing 'data'"));
    }
}
