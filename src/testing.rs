//! Test helper functions for building common message types and mock `StreamFn`.
//!
//! Previously gated behind the `test-helpers` feature; now always available so
//! both downstream crates and this crate's own integration tests can reuse them
//! without duplicating constructors.

use crate::loop_::AgentEvent;
use crate::stream::{AssistantMessageEvent, StreamErrorKind, StreamFn, StreamOptions};
use crate::tool::permissive_object_schema;
use crate::tool::{AgentTool, AgentToolResult, ToolFuture};
use crate::types::{AgentContext, ModelSpec};
use crate::types::{
    AgentMessage, AssistantMessage, ContentBlock, Cost, LlmMessage, StopReason, ToolResultMessage,
    Usage, UserMessage,
};
use futures::Stream;
use serde_json::Value;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

// ─── MockStreamFn (error-fallback scripted stream) ─────────────────────

/// A mock [`StreamFn`] that yields scripted event sequences.
///
/// Returns an error event when all responses have been consumed.
/// Delegates to [`ScriptedStreamFn::with_error_fallback`].
///
/// This is the most commonly used mock in tests — it replays canned
/// responses and fails loudly when exhausted.
pub struct MockStreamFn(ScriptedStreamFn);

impl MockStreamFn {
    /// Create a `MockStreamFn` with the given event sequences.
    ///
    /// When responses are exhausted, an error event is returned.
    #[must_use]
    pub const fn new(responses: Vec<Vec<AssistantMessageEvent>>) -> Self {
        Self(ScriptedStreamFn::with_error_fallback(responses))
    }
}

impl StreamFn for MockStreamFn {
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        self.0.stream(model, context, options, cancellation_token)
    }
}

// ─── SimpleMockStreamFn (simple token-based) ────────────────────────────

/// A deterministic [`StreamFn`] implementation for testing.
///
/// Emits the configured text tokens as a properly-sequenced event stream
/// (`Start -> TextStart -> TextDelta x N -> TextEnd -> Done`) without making
/// any network calls. Use [`SimpleMockStreamFn::new`] to configure the tokens.
pub struct SimpleMockStreamFn {
    tokens: Arc<Vec<String>>,
}

impl SimpleMockStreamFn {
    /// Create a `SimpleMockStreamFn` that will emit `tokens` in order.
    #[must_use]
    pub fn new(tokens: Vec<String>) -> Self {
        Self {
            tokens: Arc::new(tokens),
        }
    }

    /// Create a `SimpleMockStreamFn` that emits a single text response.
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        Self::new(vec![text.to_string()])
    }
}

impl StreamFn for SimpleMockStreamFn {
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
            schema: permissive_object_schema(),
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
        _state: std::sync::Arc<std::sync::RwLock<crate::SessionState>>,
        _credential: Option<crate::credential::ResolvedCredential>,
    ) -> ToolFuture<'_> {
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
        cache_hint: None,
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
        error_kind: None,
        timestamp: 0,
        cache_hint: None,
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
        cache_hint: None,
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

// ─── MockPlugin ──────────────────────────────────────────────────────────

#[cfg(feature = "plugins")]
use crate::plugin::Plugin;
#[cfg(feature = "plugins")]
use crate::policy::{PolicyContext, PolicyVerdict, PostTurnPolicy, PreTurnPolicy, TurnPolicyContext};
#[cfg(feature = "plugins")]
use std::sync::atomic::AtomicUsize;

/// A configurable mock [`Plugin`] for testing.
///
/// Consolidates the common inline mock patterns used across plugin test files.
/// Use builder methods to configure tools, policies, and event tracking.
#[cfg(feature = "plugins")]
pub struct MockPlugin {
    plugin_name: String,
    priority: i32,
    tool_names: Vec<String>,
    event_counter: Option<Arc<AtomicUsize>>,
    post_turn_tracker: Option<Arc<AtomicBool>>,
    pre_turn_order: Option<Arc<AtomicUsize>>,
    stopping_pre_turn: bool,
    init_called: Arc<AtomicBool>,
}

#[cfg(feature = "plugins")]
impl MockPlugin {
    /// Create a `MockPlugin` with the given name and default settings.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            plugin_name: name.into(),
            priority: 0,
            tool_names: vec![],
            event_counter: None,
            post_turn_tracker: None,
            pre_turn_order: None,
            stopping_pre_turn: false,
            init_called: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Set the priority for this plugin.
    #[must_use]
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Contribute tools with the given names (each wrapped with a permissive schema).
    #[must_use]
    pub fn with_tools(mut self, names: &[&str]) -> Self {
        self.tool_names = names.iter().map(|s| s.to_string()).collect();
        self
    }

    /// Attach an event counter incremented on every `on_event` call.
    #[must_use]
    pub fn with_event_counter(mut self, counter: Arc<AtomicUsize>) -> Self {
        self.event_counter = Some(counter);
        self
    }

    /// Attach a `fired` flag set to `true` when the contributed post-turn policy evaluates.
    #[must_use]
    pub fn with_post_turn_tracker(mut self, fired: Arc<AtomicBool>) -> Self {
        self.post_turn_tracker = Some(fired);
        self
    }

    /// Attach an order recorder for the contributed pre-turn policy.
    ///
    /// When the policy evaluates it fetches-and-increments a global sequence counter
    /// and stores the resulting position in `order`.
    #[must_use]
    pub fn with_pre_turn_order(mut self, order: Arc<AtomicUsize>) -> Self {
        self.pre_turn_order = Some(order);
        self
    }

    /// Make the contributed pre-turn policy return `PolicyVerdict::Stop`.
    #[must_use]
    pub fn with_stopping_pre_turn(mut self) -> Self {
        self.stopping_pre_turn = true;
        self
    }

    /// Returns a handle to the `init_called` flag — set to `true` when `on_init` fires.
    pub fn init_called(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.init_called)
    }
}

#[cfg(feature = "plugins")]
impl Plugin for MockPlugin {
    fn name(&self) -> &str {
        &self.plugin_name
    }

    fn priority(&self) -> i32 {
        self.priority
    }

    fn on_init(&self, _agent: &crate::Agent) {
        self.init_called.store(true, Ordering::SeqCst);
    }

    fn pre_turn_policies(&self) -> Vec<Arc<dyn PreTurnPolicy>> {
        if self.stopping_pre_turn {
            return vec![Arc::new(StoppingPreTurnPolicy {
                label: format!("{}-stopping", self.plugin_name),
            })];
        }
        if let Some(order) = &self.pre_turn_order {
            return vec![Arc::new(OrderRecordingPreTurnPolicy {
                label: format!("{}-pre-turn", self.plugin_name),
                order: Arc::clone(order),
            })];
        }
        vec![]
    }

    fn post_turn_policies(&self) -> Vec<Arc<dyn PostTurnPolicy>> {
        if let Some(fired) = &self.post_turn_tracker {
            return vec![Arc::new(RecordingPostTurnPolicy {
                fired: Arc::clone(fired),
            })];
        }
        vec![]
    }

    fn on_event(&self, _event: &crate::AgentEvent) {
        if let Some(counter) = &self.event_counter {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn tools(&self) -> Vec<Arc<dyn crate::tool::AgentTool>> {
        self.tool_names
            .iter()
            .map(|n| Arc::new(MockTool::new(n)) as Arc<dyn crate::tool::AgentTool>)
            .collect()
    }
}

// ─── Policy helpers used by MockPlugin ──────────────────────────────────────

/// A post-turn policy that sets a `fired` flag on evaluation.
#[cfg(feature = "plugins")]
pub struct RecordingPostTurnPolicy {
    /// Set to `true` when this policy evaluates.
    pub fired: Arc<AtomicBool>,
}

#[cfg(feature = "plugins")]
impl PostTurnPolicy for RecordingPostTurnPolicy {
    fn name(&self) -> &str {
        "recording-post-turn"
    }

    fn evaluate(&self, _ctx: &PolicyContext<'_>, _turn: &TurnPolicyContext<'_>) -> PolicyVerdict {
        self.fired.store(true, Ordering::SeqCst);
        PolicyVerdict::Continue
    }
}

/// Global sequence counter for [`OrderRecordingPreTurnPolicy`].
///
/// Tests that need to track relative evaluation order across policies
/// should reset this with `MOCK_PLUGIN_GLOBAL_ORDER.store(0, Ordering::SeqCst)`
/// at the start of each test.
#[cfg(feature = "plugins")]
pub static MOCK_PLUGIN_GLOBAL_ORDER: AtomicUsize = AtomicUsize::new(0);

/// A pre-turn policy that records its evaluation order via a global sequence counter.
#[cfg(feature = "plugins")]
pub struct OrderRecordingPreTurnPolicy {
    /// Label used as the policy name.
    pub label: String,
    /// Stores the sequence number assigned during evaluation.
    pub order: Arc<AtomicUsize>,
}

#[cfg(feature = "plugins")]
impl PreTurnPolicy for OrderRecordingPreTurnPolicy {
    fn name(&self) -> &str {
        &self.label
    }

    fn evaluate(&self, _ctx: &PolicyContext<'_>) -> PolicyVerdict {
        let seq = MOCK_PLUGIN_GLOBAL_ORDER.fetch_add(1, Ordering::SeqCst);
        self.order.store(seq, Ordering::SeqCst);
        PolicyVerdict::Continue
    }
}

/// A pre-turn policy that always returns `PolicyVerdict::Stop`.
#[cfg(feature = "plugins")]
pub struct StoppingPreTurnPolicy {
    /// Label used as the policy name.
    pub label: String,
}

#[cfg(feature = "plugins")]
impl PreTurnPolicy for StoppingPreTurnPolicy {
    fn name(&self) -> &str {
        &self.label
    }

    fn evaluate(&self, _ctx: &PolicyContext<'_>) -> PolicyVerdict {
        PolicyVerdict::Stop("stopped by policy".into())
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
        AgentEvent::StateChanged { .. } => "StateChanged".into(),
        AgentEvent::CacheAction { .. } => "CacheAction".into(),
        AgentEvent::McpServerConnected { .. } => "McpServerConnected".into(),
        AgentEvent::McpServerDisconnected { .. } => "McpServerDisconnected".into(),
        AgentEvent::McpToolsDiscovered { .. } => "McpToolsDiscovered".into(),
        AgentEvent::McpToolCallStarted { .. } => "McpToolCallStarted".into(),
        AgentEvent::McpToolCallCompleted { .. } => "McpToolCallCompleted".into(),
        #[cfg(feature = "artifact-store")]
        AgentEvent::ArtifactSaved { .. } => "ArtifactSaved".into(),
    }
}

// ─── MockPlugin (plugins feature only) ──────────────────────────────────

#[cfg(feature = "plugins")]
mod mock_plugin {
    use super::*;
    use crate::plugin::Plugin;
    use crate::policy::{PostLoopPolicy, PostTurnPolicy, PreDispatchPolicy, PreTurnPolicy};
    use std::sync::atomic::AtomicUsize;

    /// A configurable mock [`Plugin`] for testing plugin registration, ordering,
    /// and contribution merging.
    ///
    /// Tracks `on_init` invocations and records the order relative to other plugins.
    pub struct MockPlugin {
        name: String,
        priority: i32,
        tools: Vec<Arc<dyn AgentTool>>,
        pre_turn: Vec<Arc<dyn PreTurnPolicy>>,
        pre_dispatch: Vec<Arc<dyn PreDispatchPolicy>>,
        post_turn: Vec<Arc<dyn PostTurnPolicy>>,
        post_loop: Vec<Arc<dyn PostLoopPolicy>>,
        init_count: Arc<AtomicUsize>,
        init_order: Option<Arc<AtomicUsize>>,
    }

    impl MockPlugin {
        /// Create a minimal plugin with the given name and default priority 0.
        #[must_use]
        pub fn new(name: &str) -> Self {
            Self {
                name: name.to_owned(),
                priority: 0,
                tools: Vec::new(),
                pre_turn: Vec::new(),
                pre_dispatch: Vec::new(),
                post_turn: Vec::new(),
                post_loop: Vec::new(),
                init_count: Arc::new(AtomicUsize::new(0)),
                init_order: None,
            }
        }

        /// Set the priority.
        #[must_use]
        pub const fn with_priority(mut self, priority: i32) -> Self {
            self.priority = priority;
            self
        }

        /// Add a tool to the plugin's contributions.
        #[must_use]
        pub fn with_tool(mut self, tool: Arc<dyn AgentTool>) -> Self {
            self.tools.push(tool);
            self
        }

        /// Add a pre-turn policy.
        #[must_use]
        pub fn with_pre_turn_policy(mut self, policy: Arc<dyn PreTurnPolicy>) -> Self {
            self.pre_turn.push(policy);
            self
        }

        /// Add a pre-dispatch policy.
        #[must_use]
        pub fn with_pre_dispatch_policy(mut self, policy: Arc<dyn PreDispatchPolicy>) -> Self {
            self.pre_dispatch.push(policy);
            self
        }

        /// Add a post-turn policy.
        #[must_use]
        pub fn with_post_turn_policy(mut self, policy: Arc<dyn PostTurnPolicy>) -> Self {
            self.post_turn.push(policy);
            self
        }

        /// Add a post-loop policy.
        #[must_use]
        pub fn with_post_loop_policy(mut self, policy: Arc<dyn PostLoopPolicy>) -> Self {
            self.post_loop.push(policy);
            self
        }

        /// Provide a shared counter for tracking init order across plugins.
        #[must_use]
        pub fn with_init_order(mut self, order: Arc<AtomicUsize>) -> Self {
            self.init_order = Some(order);
            self
        }

        /// Number of times `on_init` was called.
        pub fn init_count(&self) -> usize {
            self.init_count.load(Ordering::SeqCst)
        }
    }

    impl Plugin for MockPlugin {
        fn name(&self) -> &str {
            &self.name
        }

        fn priority(&self) -> i32 {
            self.priority
        }

        fn on_init(&self, _agent: &crate::Agent) {
            self.init_count.fetch_add(1, Ordering::SeqCst);
            if let Some(order) = &self.init_order {
                order.fetch_add(1, Ordering::SeqCst);
            }
        }

        fn pre_turn_policies(&self) -> Vec<Arc<dyn PreTurnPolicy>> {
            self.pre_turn.clone()
        }

        fn pre_dispatch_policies(&self) -> Vec<Arc<dyn PreDispatchPolicy>> {
            self.pre_dispatch.clone()
        }

        fn post_turn_policies(&self) -> Vec<Arc<dyn PostTurnPolicy>> {
            self.post_turn.clone()
        }

        fn post_loop_policies(&self) -> Vec<Arc<dyn PostLoopPolicy>> {
            self.post_loop.clone()
        }

        fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
            self.tools.clone()
        }
    }
}

#[cfg(feature = "plugins")]
pub use mock_plugin::MockPlugin;
