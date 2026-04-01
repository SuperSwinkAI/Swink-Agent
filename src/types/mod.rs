//! Foundation types for the swink agent.
//!
//! This module defines every type that crosses a public boundary in the harness.
//! All other modules depend on it; it depends on nothing else in the crate.

mod custom_message;
mod model;

pub use custom_message::*;
pub use model::*;

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
#[non_exhaustive]
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
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    /// Base64-encoded image data with a media type.
    Base64 { media_type: String, data: String },

    /// A URL pointing to an image.
    Url { url: String, media_type: String },

    /// A local file path pointing to an image.
    File {
        path: std::path::PathBuf,
        media_type: String,
    },
}

// ─── Messages ───────────────────────────────────────────────────────────────

/// A message from the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMessage {
    pub content: Vec<ContentBlock>,
    pub timestamp: u64,
    /// Provider-agnostic cache hint for this message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_hint: Option<crate::context_cache::CacheHint>,
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
    /// Provider-agnostic cache hint for this message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_hint: Option<crate::context_cache::CacheHint>,
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
    /// Provider-agnostic cache hint for this message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_hint: Option<crate::context_cache::CacheHint>,
}

/// A discriminated union of the three LLM message roles.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum LlmMessage {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
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
        *self += other.clone();
    }
}

impl Add for Usage {
    type Output = Self;

    fn add(mut self, rhs: Self) -> Self::Output {
        self += rhs;
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
        self += rhs;
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
#[non_exhaustive]
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

impl AgentResult {
    /// Extract the text content from the last assistant message, if any.
    ///
    /// Iterates messages in reverse order, finds the first `Assistant` message,
    /// and returns its concatenated text blocks. Returns an empty string if no
    /// assistant message is found or if the assistant message contains no text.
    pub fn assistant_text(&self) -> String {
        self.messages
            .iter()
            .rev()
            .find_map(|msg| match msg {
                AgentMessage::Llm(LlmMessage::Assistant(a)) => Some(a),
                _ => None,
            })
            .map(|a| ContentBlock::extract_text(&a.content))
            .unwrap_or_default()
    }
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
    /// Session state changes during this turn, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_delta: Option<crate::StateDelta>,
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
    assert_send_sync::<crate::error::DowncastError>();
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
        let downcasted = restored
            .as_any()
            .downcast_ref::<MockNotification>()
            .unwrap();
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
        assert!(!registry.has_type_name("mock_notification"));
        registry.register_type::<MockNotification>("mock_notification");
        assert!(registry.has_type_name("mock_notification"));
    }

    #[test]
    fn assistant_text_extracts_last_assistant_message() {
        let result = AgentResult {
            messages: vec![
                AgentMessage::Llm(LlmMessage::User(UserMessage {
                    content: vec![ContentBlock::Text {
                        text: "hi".to_string(),
                    }],
                    timestamp: 0,
                    cache_hint: None,
                })),
                AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
                    content: vec![ContentBlock::Text {
                        text: "first".to_string(),
                    }],
                    provider: "test".to_string(),
                    model_id: "m".to_string(),
                    usage: Usage::default(),
                    cost: Cost::default(),
                    stop_reason: StopReason::Stop,
                    error_message: None,
                    timestamp: 0,
                    cache_hint: None,
                })),
                AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
                    content: vec![ContentBlock::Text {
                        text: "second".to_string(),
                    }],
                    provider: "test".to_string(),
                    model_id: "m".to_string(),
                    usage: Usage::default(),
                    cost: Cost::default(),
                    stop_reason: StopReason::Stop,
                    error_message: None,
                    timestamp: 0,
                    cache_hint: None,
                })),
            ],
            stop_reason: StopReason::Stop,
            usage: Usage::default(),
            cost: Cost::default(),
            error: None,
        };
        assert_eq!(result.assistant_text(), "second");
    }

    #[test]
    fn assistant_text_returns_empty_when_no_assistant() {
        let result = AgentResult {
            messages: vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "hi".to_string(),
                }],
                timestamp: 0,
                cache_hint: None,
            }))],
            stop_reason: StopReason::Stop,
            usage: Usage::default(),
            cost: Cost::default(),
            error: None,
        };
        assert_eq!(result.assistant_text(), "");
    }

    #[test]
    fn assistant_text_returns_empty_when_no_messages() {
        let result = AgentResult {
            messages: vec![],
            stop_reason: StopReason::Stop,
            usage: Usage::default(),
            cost: Cost::default(),
            error: None,
        };
        assert_eq!(result.assistant_text(), "");
    }

    #[test]
    fn deserialize_custom_message_missing_fields() {
        let registry = CustomMessageRegistry::new();

        let no_type = serde_json::json!({"data": {}});
        assert!(
            deserialize_custom_message(&registry, &no_type)
                .unwrap_err()
                .contains("missing 'type'")
        );

        let no_data = serde_json::json!({"type": "foo"});
        assert!(
            deserialize_custom_message(&registry, &no_data)
                .unwrap_err()
                .contains("missing 'data'")
        );
    }
}
