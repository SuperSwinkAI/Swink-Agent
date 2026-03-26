//! Test helper functions for building common message types and mock `StreamFn`.
//!
//! Previously gated behind the `test-helpers` feature; now always available so
//! both downstream crates and this crate's own integration tests can reuse them
//! without duplicating constructors.

use crate::loop_::AgentEvent;
use crate::stream::{AssistantMessageEvent, StreamErrorKind, StreamFn, StreamOptions};
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
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

// ─── MockStreamFn (simple token-based) ──────────────────────────────────

/// A deterministic [`StreamFn`] implementation for testing.
///
/// Emits the configured text tokens as a properly-sequenced event stream
/// (`Start -> TextStart -> TextDelta x N -> TextEnd -> Done`) without making
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
        let events = text_only_events_multi((*self.tokens).clone());
        Box::pin(futures::stream::iter(events))
    }
}

// ─── ScriptedStreamFn ────────────────────────────────────────────────────

/// A deterministic [`StreamFn`] that replays scripted event sequences.
///
/// Each call to `stream()` pops the next event sequence from the queue.
/// If the queue is empty, returns a default text response (configurable
/// via [`ScriptedStreamFn::with_error_fallback`]).
pub struct ScriptedStreamFn {
    responses: Mutex<Vec<Vec<AssistantMessageEvent>>>,
    use_error_fallback: bool,
}

impl ScriptedStreamFn {
    /// Create a new `ScriptedStreamFn` with the given event sequences.
    ///
    /// When responses are exhausted, a default text reply is returned.
    #[must_use]
    pub const fn new(responses: Vec<Vec<AssistantMessageEvent>>) -> Self {
        Self {
            responses: Mutex::new(responses),
            use_error_fallback: false,
        }
    }

    /// Create a `ScriptedStreamFn` that returns an error event when
    /// responses are exhausted (instead of a default text reply).
    #[must_use]
    pub const fn with_error_fallback(responses: Vec<Vec<AssistantMessageEvent>>) -> Self {
        Self {
            responses: Mutex::new(responses),
            use_error_fallback: true,
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
        let fallback = if self.use_error_fallback {
            default_exhausted_fallback()
        } else {
            text_only_events_multi(vec!["default response".to_string()])
        };
        let events = next_response(&self.responses, fallback);
        Box::pin(futures::stream::iter(events))
    }
}

// ─── MockFlagStreamFn ────────────────────────────────────────────────────

/// A stream function that sets a flag when called — useful for verifying
/// which `StreamFn` was invoked.
#[allow(dead_code)]
pub struct MockFlagStreamFn {
    pub called: AtomicBool,
    pub responses: Mutex<Vec<Vec<AssistantMessageEvent>>>,
}

impl StreamFn for MockFlagStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a ModelSpec,
        _context: &'a AgentContext,
        _options: &'a StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        self.called.store(true, Ordering::SeqCst);
        let events = next_response(&self.responses, text_events("fallback"));
        Box::pin(futures::stream::iter(events))
    }
}

// ─── MockContextCapturingStreamFn ────────────────────────────────────────

/// A mock `StreamFn` that captures the number of messages passed in each call.
#[allow(dead_code)]
pub struct MockContextCapturingStreamFn {
    pub responses: Mutex<Vec<Vec<AssistantMessageEvent>>>,
    pub captured_message_counts: Mutex<Vec<usize>>,
}

#[allow(dead_code)]
impl MockContextCapturingStreamFn {
    pub const fn new(responses: Vec<Vec<AssistantMessageEvent>>) -> Self {
        Self {
            responses: Mutex::new(responses),
            captured_message_counts: Mutex::new(Vec::new()),
        }
    }
}

impl StreamFn for MockContextCapturingStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a ModelSpec,
        context: &'a AgentContext,
        _options: &'a StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn futures::Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        self.captured_message_counts
            .lock()
            .unwrap()
            .push(context.messages.len());
        let events = next_response(&self.responses, default_exhausted_fallback());
        Box::pin(futures::stream::iter(events))
    }
}

// ─── MockApiKeyCapturingStreamFn ─────────────────────────────────────────

/// A mock `StreamFn` that captures resolved API keys from stream options.
#[allow(dead_code)]
pub struct MockApiKeyCapturingStreamFn {
    pub responses: Mutex<Vec<Vec<AssistantMessageEvent>>>,
    pub captured_api_keys: Mutex<Vec<Option<String>>>,
}

#[allow(dead_code)]
impl MockApiKeyCapturingStreamFn {
    pub const fn new(responses: Vec<Vec<AssistantMessageEvent>>) -> Self {
        Self {
            responses: Mutex::new(responses),
            captured_api_keys: Mutex::new(Vec::new()),
        }
    }
}

impl StreamFn for MockApiKeyCapturingStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a ModelSpec,
        _context: &'a AgentContext,
        options: &'a StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn futures::Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        self.captured_api_keys
            .lock()
            .unwrap()
            .push(options.api_key.clone());
        let events = next_response(&self.responses, default_exhausted_fallback());
        Box::pin(futures::stream::iter(events))
    }
}

// ─── MockTool ────────────────────────────────────────────────────────────

/// A configurable mock [`AgentTool`] for testing.
///
/// Returns a fixed result text response by default. Use builder methods
/// to configure delay, approval requirement, schema, etc.
pub struct MockTool {
    tool_name: String,
    schema: Value,
    result: Mutex<Option<AgentToolResult>>,
    delay: Option<Duration>,
    executed: Arc<AtomicBool>,
    execute_count: Arc<AtomicU32>,
    approval_required: bool,
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
            result: Mutex::new(Some(AgentToolResult::text("mock result"))),
            delay: None,
            executed: Arc::new(AtomicBool::new(false)),
            execute_count: Arc::new(AtomicU32::new(0)),
            approval_required: false,
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

    /// Add an artificial delay to `execute()`.
    #[must_use]
    pub const fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }

    /// Set whether this tool requires approval.
    #[must_use]
    pub const fn with_requires_approval(mut self, required: bool) -> Self {
        self.approval_required = required;
        self
    }

    /// Returns `true` if `execute()` has been called at least once.
    pub fn was_executed(&self) -> bool {
        self.executed.load(Ordering::SeqCst)
    }

    /// Returns the number of times `execute()` has been called.
    pub fn execution_count(&self) -> u32 {
        self.execute_count.load(Ordering::SeqCst)
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

    fn requires_approval(&self) -> bool {
        self.approval_required
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        _params: Value,
        cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
    ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
        self.executed.store(true, Ordering::SeqCst);
        self.execute_count.fetch_add(1, Ordering::SeqCst);
        let result = self
            .result
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| AgentToolResult::text("mock result"));
        let delay = self.delay;
        Box::pin(async move {
            if let Some(d) = delay {
                tokio::select! {
                    () = tokio::time::sleep(d) => {}
                    () = cancellation_token.cancelled() => {
                        return AgentToolResult::text("cancelled");
                    }
                }
            }
            result
        })
    }
}

// ─── Event helper functions ──────────────────────────────────────────────

/// Build a well-formed event sequence for a single text string.
///
/// Produces: `Start -> TextStart{0} -> TextDelta{0, text} -> TextEnd{0} -> Done`.
#[must_use]
pub fn text_only_events(text: &str) -> Vec<AssistantMessageEvent> {
    text_only_events_multi(vec![text.to_string()])
}

/// Alias for [`text_only_events`] — same function, alternative name.
#[must_use]
pub fn text_events(text: &str) -> Vec<AssistantMessageEvent> {
    text_only_events(text)
}

/// Build a well-formed event sequence for a plain-text response with multiple tokens.
///
/// Produces: `Start -> TextStart{0} -> TextDelta{0, t} for each t -> TextEnd{0} -> Done`.
#[must_use]
pub fn text_only_events_multi(tokens: Vec<String>) -> Vec<AssistantMessageEvent> {
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

/// Build events for a single tool call response.
#[allow(dead_code)]
#[must_use]
pub fn tool_call_events(id: &str, name: &str, args: &str) -> Vec<AssistantMessageEvent> {
    vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            id: id.to_string(),
            name: name.to_string(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            delta: args.to_string(),
        },
        AssistantMessageEvent::ToolCallEnd { content_index: 0 },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
            cost: Cost::default(),
        },
    ]
}

/// Build events for multiple tool calls in a single response.
///
/// Each entry is `(id, name, args)`.
#[allow(dead_code)]
#[must_use]
pub fn tool_call_events_multi(calls: &[(&str, &str, &str)]) -> Vec<AssistantMessageEvent> {
    let mut events = vec![AssistantMessageEvent::Start];
    for (i, (id, name, args)) in calls.iter().enumerate() {
        events.push(AssistantMessageEvent::ToolCallStart {
            content_index: i,
            id: id.to_string(),
            name: name.to_string(),
        });
        events.push(AssistantMessageEvent::ToolCallDelta {
            content_index: i,
            delta: args.to_string(),
        });
        events.push(AssistantMessageEvent::ToolCallEnd { content_index: i });
    }
    events.push(AssistantMessageEvent::Done {
        stop_reason: StopReason::ToolUse,
        usage: Usage::default(),
        cost: Cost::default(),
    });
    events
}

/// Build events for an error response.
#[allow(dead_code)]
#[must_use]
pub fn error_events(
    message: &str,
    error_kind: Option<StreamErrorKind>,
) -> Vec<AssistantMessageEvent> {
    vec![AssistantMessageEvent::Error {
        stop_reason: StopReason::Error,
        error_message: message.to_string(),
        usage: None,
        error_kind,
    }]
}

// ─── Message helper functions ────────────────────────────────────────────

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

// ─── Model / convert helpers ─────────────────────────────────────────────

/// Default model spec for tests.
#[allow(dead_code)]
#[must_use]
pub fn default_model() -> ModelSpec {
    ModelSpec::new("test", "test-model")
}

/// Default message converter for tests — passes through LLM messages, drops custom.
#[allow(dead_code)]
pub fn default_convert(msg: &AgentMessage) -> Option<LlmMessage> {
    match msg {
        AgentMessage::Llm(llm) => Some(llm.clone()),
        AgentMessage::Custom(_) => None,
    }
}

// ─── Response queue helpers ──────────────────────────────────────────────

/// Pops the next scripted response from a `Mutex<Vec<…>>`, returning the
/// fallback when the list is exhausted. Used by scripted `StreamFn` mocks
/// to avoid duplicating the pop-or-fallback pattern.
#[allow(dead_code)]
pub fn next_response(
    responses: &Mutex<Vec<Vec<AssistantMessageEvent>>>,
    fallback: Vec<AssistantMessageEvent>,
) -> Vec<AssistantMessageEvent> {
    let mut guard = responses.lock().unwrap();
    if guard.is_empty() {
        fallback
    } else {
        guard.remove(0)
    }
}

/// Default error fallback used by most mock `StreamFn` implementations.
#[allow(dead_code)]
#[must_use]
pub fn default_exhausted_fallback() -> Vec<AssistantMessageEvent> {
    vec![AssistantMessageEvent::Error {
        stop_reason: StopReason::Error,
        error_message: "no more scripted responses".to_string(),
        usage: None,
        error_kind: None,
    }]
}

// ─── EventCollector ──────────────────────────────────────────────────────

/// Subscribes to Agent events and collects them for assertion.
#[allow(dead_code)]
#[derive(Clone)]
pub struct EventCollector {
    events: Arc<Mutex<Vec<String>>>,
}

#[allow(dead_code)]
impl Default for EventCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)]
impl EventCollector {
    pub fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Returns a closure suitable for `agent.subscribe(...)`.
    pub fn subscriber(&self) -> impl Fn(&AgentEvent) + Send + Sync + 'static {
        let events = Arc::clone(&self.events);
        move |event: &AgentEvent| {
            let name = event_variant_name(event);
            events.lock().unwrap().push(name);
        }
    }

    /// Snapshot of collected event names.
    pub fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }

    /// Number of events collected.
    pub fn count(&self) -> usize {
        self.events.lock().unwrap().len()
    }

    /// Position of first occurrence of an event name.
    pub fn position(&self, name: &str) -> Option<usize> {
        self.events().iter().position(|n| n == name)
    }
}

/// Extract the variant name from an `AgentEvent`.
#[allow(dead_code)]
pub fn event_variant_name(event: &AgentEvent) -> String {
    match event {
        AgentEvent::AgentStart => "AgentStart".into(),
        AgentEvent::AgentEnd { .. } => "AgentEnd".into(),
        AgentEvent::TurnStart => "TurnStart".into(),
        AgentEvent::TurnEnd { .. } => "TurnEnd".into(),
        AgentEvent::MessageStart => "MessageStart".into(),
        AgentEvent::MessageUpdate { .. } => "MessageUpdate".into(),
        AgentEvent::MessageEnd { .. } => "MessageEnd".into(),
        AgentEvent::ToolExecutionStart { .. } => "ToolExecutionStart".into(),
        AgentEvent::ToolExecutionUpdate { .. } => "ToolExecutionUpdate".into(),
        AgentEvent::ToolExecutionEnd { .. } => "ToolExecutionEnd".into(),
        AgentEvent::ToolApprovalRequested { .. } => "ToolApprovalRequested".into(),
        AgentEvent::ToolApprovalResolved { .. } => "ToolApprovalResolved".into(),
        AgentEvent::BeforeLlmCall { .. } => "BeforeLlmCall".into(),
        AgentEvent::ContextCompacted { .. } => "ContextCompacted".into(),
        AgentEvent::Custom(emission) => format!("Custom({})", emission.name),
        AgentEvent::ModelFallback { .. } => "ModelFallback".into(),
        AgentEvent::ModelCycled { .. } => "ModelCycled".into(),
    }
}
