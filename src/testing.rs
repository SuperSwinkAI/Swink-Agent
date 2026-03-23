//! Test helper functions for building common message types and mock `StreamFn`.
//!
//! Gated behind the `test-helpers` feature so downstream crates can
//! reuse them in their own test suites without duplicating constructors.

use crate::stream::{AssistantMessageEvent, StreamFn, StreamOptions};
use crate::tool::{AgentTool, AgentToolResult};
use crate::types::{AgentContext, ModelSpec};
use crate::types::{
    AgentMessage, AssistantMessage, ContentBlock, Cost, LlmMessage, StopReason, ToolResultMessage,
    Usage, UserMessage,
};
use futures::Stream;
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
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

    /// Create a `MockStreamFn` that emits a single text response.
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        Self::new(vec![text.to_string()])
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

// ─── ScriptedStreamFn ─────────────────────────────────────────────────────

/// A deterministic [`StreamFn`] that replays scripted event sequences.
///
/// Each call to `stream()` pops the next event sequence from the queue.
/// If the queue is empty, returns a default text response.
pub struct ScriptedStreamFn {
    responses: std::sync::Mutex<Vec<Vec<AssistantMessageEvent>>>,
}

impl ScriptedStreamFn {
    /// Create a new `ScriptedStreamFn` with the given event sequences.
    #[must_use]
    pub fn new(responses: Vec<Vec<AssistantMessageEvent>>) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses),
        }
    }
}

impl StreamFn for ScriptedStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a ModelSpec,
        _context: &'a AgentContext,
        _options: &'a StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        let events = {
            let mut queue = self
                .responses
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if queue.is_empty() {
                text_only_events(vec!["default response".to_string()])
            } else {
                queue.remove(0)
            }
        };
        Box::pin(futures::stream::iter(events))
    }
}

// ─── text_events convenience ─────────────────────────────────────────────

/// Build a well-formed event sequence for a single text string.
///
/// Convenience wrapper around [`text_only_events`] for the common case
/// of a single text token.
#[must_use]
pub fn text_events(text: &str) -> Vec<AssistantMessageEvent> {
    text_only_events(vec![text.to_string()])
}

// ─── MockTool ────────────────────────────────────────────────────────────

/// A simple mock [`AgentTool`] for downstream crate tests.
///
/// Returns a fixed `"mock result"` text response. Use this when you need
/// a tool that satisfies the trait but whose specific behaviour is not
/// under test.
pub struct MockTool {
    tool_name: String,
    schema: Value,
    result: Mutex<Option<AgentToolResult>>,
}

impl MockTool {
    /// Create a `MockTool` with the given name and an empty-object schema.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            tool_name: name.to_string(),
            schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": true
            }),
            result: Mutex::new(None),
        }
    }

    /// Override the schema returned by [`AgentTool::parameters_schema`].
    #[must_use]
    pub fn with_schema(mut self, schema: Value) -> Self {
        self.schema = schema;
        self
    }

    /// Override the result returned by [`AgentTool::execute`].
    #[must_use]
    pub fn with_result(self, result: AgentToolResult) -> Self {
        *self.result.lock().unwrap() = Some(result);
        self
    }
}

impl AgentTool for MockTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn label(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &'static str {
        "A mock tool for testing"
    }

    fn parameters_schema(&self) -> &Value {
        &self.schema
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        _params: Value,
        _cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
    ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
        let result = self
            .result
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| AgentToolResult::text("mock result"));
        Box::pin(async move { result })
    }
}
