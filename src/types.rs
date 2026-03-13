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
pub trait CustomMessage: Send + Sync + fmt::Debug + std::any::Any {
    /// Downcast helper. Returns `self` as `&dyn Any` for type-safe downcasting.
    fn as_any(&self) -> &dyn std::any::Any;
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
    assert_send_sync::<ModelSpec>();
    assert_send_sync::<AgentResult>();
    assert_send_sync::<AgentContext>();
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
}
