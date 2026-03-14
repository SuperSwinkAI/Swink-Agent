//! Test helper functions for building common message types and mock StreamFn.
//!
//! Gated behind the `test-helpers` feature so downstream crates can
//! reuse them in their own test suites without duplicating constructors.

use crate::types::{
    AgentMessage, AssistantMessage, ContentBlock, Cost, LlmMessage, StopReason, ToolResultMessage,
    Usage, UserMessage,
};
use crate::stream::{AssistantMessageEvent, StreamFn, StreamOptions};
use crate::types::{AgentContext, ModelSpec};
use std::pin::Pin;
use std::sync::Arc;
use futures::Stream;
use tokio_util::sync::CancellationToken;

// ─── MockStreamFn ─────────────────────────────────────────────────────────

/// A deterministic [`StreamFn`] implementation for testing.
///
/// Emits the configured text tokens as a properly-sequenced event stream
/// (`Start → TextStart → TextDelta × N → TextEnd → Done`) without making
/// any network calls. Use [`MockStreamFn::new`] to configure the tokens.
pub struct MockStreamFn {
    tokens: Arc<Vec<String>>,
}

impl MockStreamFn {
    /// Create a `MockStreamFn` that will emit `tokens` in order.
    #[must_use]
    pub fn new(tokens: Vec<String>) -> Self {
        Self {
            tokens: Arc::new(tokens),
        }
    }
}

impl StreamFn for MockStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a ModelSpec,
        _context: &'a AgentContext,
        _options: &'a StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        let events = text_only_events((*self.tokens).clone());
        Box::pin(futures::stream::iter(events))
    }
}

/// Build a well-formed event sequence for a plain-text response.
///
/// Produces: `Start → TextStart{0} → TextDelta{0, t} for each t → TextEnd{0} → Done`.
#[must_use]
pub fn text_only_events(tokens: Vec<String>) -> Vec<AssistantMessageEvent> {
    let mut events = Vec::with_capacity(tokens.len() + 4);
    events.push(AssistantMessageEvent::Start);
    events.push(AssistantMessageEvent::TextStart { content_index: 0 });
    for token in tokens {
        events.push(AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: token,
        });
    }
    events.push(AssistantMessageEvent::TextEnd { content_index: 0 });
    events.push(AssistantMessageEvent::Done {
        stop_reason: StopReason::Stop,
        usage: Usage::default(),
        cost: Cost::default(),
    });
    events
}

/// Build a single [`AgentMessage::Llm`] user message with the given text.
pub fn user_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        timestamp: 0,
    }))
}

/// Build a single [`AgentMessage::Llm`] assistant message with the given text.
pub fn assistant_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        provider: String::new(),
        model_id: String::new(),
        usage: Usage::default(),
        cost: Cost::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    }))
}

/// Build a single [`AgentMessage::Llm`] tool-result message.
pub fn tool_result_msg(id: &str, text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::ToolResult(ToolResultMessage {
        tool_call_id: id.to_string(),
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        is_error: false,
        timestamp: 0,
        details: serde_json::Value::Null,
    }))
}
