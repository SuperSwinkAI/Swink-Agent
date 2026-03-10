//! Foundation types for the agent harness.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
}

impl Usage {
    /// Merge another `Usage` into this one by summing all fields.
    pub const fn merge(&mut self, other: &Self) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.total += other.total;
    }
}

impl Add for Usage {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self {
            input: self.input + rhs.input,
            output: self.output + rhs.output,
            cache_read: self.cache_read + rhs.cache_read,
            cache_write: self.cache_write + rhs.cache_write,
            total: self.total + rhs.total,
        }
    }
}

impl AddAssign for Usage {
    fn add_assign(&mut self, rhs: Self) {
        self.merge(&rhs);
    }
}

/// Per-category and total cost breakdown (floating-point currency values).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Cost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub total: f64,
}

impl Add for Cost {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self {
            input: self.input + rhs.input,
            output: self.output + rhs.output,
            cache_read: self.cache_read + rhs.cache_read,
            cache_write: self.cache_write + rhs.cache_write,
            total: self.total + rhs.total,
        }
    }
}

impl AddAssign for Cost {
    fn add_assign(&mut self, rhs: Self) {
        self.input += rhs.input;
        self.output += rhs.output;
        self.cache_read += rhs.cache_read;
        self.cache_write += rhs.cache_write;
        self.total += rhs.total;
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
    pub const fn new(budgets: HashMap<ThinkingLevel, u64>) -> Self {
        Self { budgets }
    }

    /// Look up the token budget for a given thinking level.
    pub fn get(&self, level: &ThinkingLevel) -> Option<u64> {
        self.budgets.get(level).copied()
    }
}

/// Identifies the target model for a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSpec {
    pub provider: String,
    pub model_id: String,
    pub thinking_level: ThinkingLevel,
    pub thinking_budgets: Option<ThinkingBudgets>,
}

impl ModelSpec {
    /// Create a new `ModelSpec` with thinking disabled and no budgets.
    pub fn new(provider: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model_id: model_id.into(),
            thinking_level: ThinkingLevel::Off,
            thinking_budgets: None,
        }
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
    /// Optional error string if the run ended in an error state.
    pub error: Option<String>,
}

impl fmt::Debug for AgentResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentResult")
            .field("messages", &self.messages)
            .field("stop_reason", &self.stop_reason)
            .field("usage", &self.usage)
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
    /// Placeholder for tool trait objects. Uses `Any` until the `AgentTool`
    /// trait is defined in Phase 2.
    pub tools: Vec<Arc<dyn std::any::Any + Send + Sync>>,
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

    // ── 1.2: ContentBlock variants construct and pattern-match ──

    #[test]
    fn content_block_text() {
        let block = ContentBlock::Text {
            text: "hello".into(),
        };
        assert!(matches!(block, ContentBlock::Text { text } if text == "hello"));
    }

    #[test]
    fn content_block_thinking() {
        let block = ContentBlock::Thinking {
            thinking: "reason".into(),
            signature: Some("sig".into()),
        };
        assert!(
            matches!(&block, ContentBlock::Thinking { thinking, signature }
                if thinking == "reason" && signature.as_deref() == Some("sig"))
        );
    }

    #[test]
    fn content_block_tool_call() {
        let block = ContentBlock::ToolCall {
            id: "tc_1".into(),
            name: "read".into(),
            arguments: serde_json::json!({"path": "/tmp"}),
            partial_json: None,
        };
        assert!(
            matches!(&block, ContentBlock::ToolCall { id, name, .. } if id == "tc_1" && name == "read")
        );
    }

    #[test]
    fn content_block_image() {
        let block = ContentBlock::Image {
            source: ImageSource::Url {
                url: "https://example.com/img.png".into(),
            },
        };
        assert!(
            matches!(&block, ContentBlock::Image { source: ImageSource::Url { url } } if url.contains("example"))
        );
    }

    // ── 1.3: LlmMessage wraps/unwraps each message type ──

    #[test]
    fn llm_message_user() {
        let msg = LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text { text: "hi".into() }],
            timestamp: 1,
        });
        assert!(matches!(msg, LlmMessage::User(_)));
    }

    #[test]
    fn llm_message_assistant() {
        let msg = LlmMessage::Assistant(AssistantMessage {
            content: vec![],
            provider: "anthropic".into(),
            model_id: "claude".into(),
            usage: Usage::default(),
            cost: Cost::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 2,
        });
        assert!(matches!(msg, LlmMessage::Assistant(_)));
    }

    #[test]
    fn llm_message_tool_result() {
        let msg = LlmMessage::ToolResult(ToolResultMessage {
            tool_call_id: "tc_1".into(),
            content: vec![ContentBlock::Text { text: "ok".into() }],
            is_error: false,
            timestamp: 3,
        });
        assert!(matches!(msg, LlmMessage::ToolResult(_)));
    }

    // ── 1.4: AgentMessage::Custom holds a boxed trait object and downcasts ──

    #[derive(Debug)]
    struct TestCustomMessage {
        value: String,
    }

    impl CustomMessage for TestCustomMessage {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[test]
    fn agent_message_custom_downcast() {
        let custom = TestCustomMessage {
            value: "hello".into(),
        };
        let msg = AgentMessage::Custom(Box::new(custom));

        if let AgentMessage::Custom(ref boxed) = msg {
            let downcasted = boxed.as_any().downcast_ref::<TestCustomMessage>();
            assert!(downcasted.is_some());
            assert_eq!(downcasted.unwrap().value, "hello");
        } else {
            panic!("expected Custom variant");
        }
    }

    // ── 1.5: Usage and Cost aggregate correctly ──

    #[test]
    fn usage_add() {
        let a = Usage {
            input: 10,
            output: 20,
            cache_read: 5,
            cache_write: 3,
            total: 38,
        };
        let b = Usage {
            input: 1,
            output: 2,
            cache_read: 3,
            cache_write: 4,
            total: 10,
        };
        let c = a + b;
        assert_eq!(c.input, 11);
        assert_eq!(c.output, 22);
        assert_eq!(c.cache_read, 8);
        assert_eq!(c.cache_write, 7);
        assert_eq!(c.total, 48);
    }

    #[test]
    fn usage_add_assign() {
        let mut a = Usage::default();
        let b = Usage {
            input: 5,
            output: 10,
            cache_read: 1,
            cache_write: 2,
            total: 18,
        };
        a += b;
        assert_eq!(a, b);
    }

    #[test]
    fn usage_merge() {
        let mut a = Usage {
            input: 1,
            output: 1,
            cache_read: 1,
            cache_write: 1,
            total: 4,
        };
        let b = Usage {
            input: 2,
            output: 2,
            cache_read: 2,
            cache_write: 2,
            total: 8,
        };
        a.merge(&b);
        assert_eq!(a.input, 3);
        assert_eq!(a.total, 12);
    }

    #[test]
    fn cost_add() {
        let a = Cost {
            input: 0.01,
            output: 0.02,
            cache_read: 0.005,
            cache_write: 0.003,
            total: 0.038,
        };
        let b = Cost {
            input: 0.01,
            output: 0.02,
            cache_read: 0.005,
            cache_write: 0.003,
            total: 0.038,
        };
        let c = a + b;
        assert!((c.input - 0.02).abs() < f64::EPSILON);
        assert!((c.total - 0.076).abs() < f64::EPSILON);
    }

    #[test]
    fn cost_add_assign() {
        let mut a = Cost::default();
        let b = Cost {
            input: 0.1,
            output: 0.2,
            cache_read: 0.0,
            cache_write: 0.0,
            total: 0.3,
        };
        a += b;
        assert!((a.total - 0.3).abs() < f64::EPSILON);
    }

    // ── 1.6: StopReason and ThinkingLevel round-trip through serde ──

    #[test]
    fn stop_reason_serde_roundtrip() {
        for reason in [
            StopReason::Stop,
            StopReason::Length,
            StopReason::ToolUse,
            StopReason::Aborted,
            StopReason::Error,
        ] {
            let json = serde_json::to_string(&reason).unwrap();
            let parsed: StopReason = serde_json::from_str(&json).unwrap();
            assert_eq!(reason, parsed);
        }
    }

    #[test]
    fn thinking_level_serde_roundtrip() {
        for level in [
            ThinkingLevel::Off,
            ThinkingLevel::Minimal,
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
            ThinkingLevel::ExtraHigh,
        ] {
            let json = serde_json::to_string(&level).unwrap();
            let parsed: ThinkingLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(level, parsed);
        }
    }

    // ── 1.7: ModelSpec constructs with defaults ──

    #[test]
    fn model_spec_defaults() {
        let spec = ModelSpec::new("anthropic", "claude-sonnet-4-6");
        assert_eq!(spec.provider, "anthropic");
        assert_eq!(spec.model_id, "claude-sonnet-4-6");
        assert_eq!(spec.thinking_level, ThinkingLevel::Off);
        assert!(spec.thinking_budgets.is_none());
    }

    // ── 1.10: AgentContext compiles with Vec<AgentMessage> and Vec<Arc<dyn Any>> ──

    #[test]
    fn agent_context_compiles() {
        let ctx = AgentContext {
            system_prompt: "You are helpful.".into(),
            messages: vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
                timestamp: 0,
            }))],
            tools: vec![],
        };
        assert_eq!(ctx.messages.len(), 1);
        assert!(ctx.tools.is_empty());
    }
}
