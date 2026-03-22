//! Shared test mocks and helpers used across integration test files.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use futures::Stream;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use swink_agent::{
    AgentContext, AgentMessage, AgentTool, AgentToolResult, AssistantMessageEvent, ContentBlock,
    Cost, LlmMessage, ModelSpec, StopReason, StreamFn, StreamOptions, Usage, UserMessage,
};

// ─── FlagStreamFn ─────────────────────────────────────────────────────────

/// A stream function that sets a flag when called — useful for verifying
/// which `StreamFn` was invoked.
#[allow(dead_code)]
pub struct FlagStreamFn {
    pub called: AtomicBool,
    pub responses: Mutex<Vec<Vec<AssistantMessageEvent>>>,
}

impl StreamFn for FlagStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a ModelSpec,
        _context: &'a swink_agent::AgentContext,
        _options: &'a StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        self.called.store(true, Ordering::SeqCst);
        let events = {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                text_only_events("fallback")
            } else {
                responses.remove(0)
            }
        };
        Box::pin(futures::stream::iter(events))
    }
}

// ─── MockStreamFn ────────────────────────────────────────────────────────

/// A mock `StreamFn` that yields scripted event sequences.
#[allow(dead_code)]
pub struct MockStreamFn {
    pub responses: Mutex<Vec<Vec<AssistantMessageEvent>>>,
}

#[allow(dead_code)]
impl MockStreamFn {
    pub const fn new(responses: Vec<Vec<AssistantMessageEvent>>) -> Self {
        Self {
            responses: Mutex::new(responses),
        }
    }
}

impl StreamFn for MockStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a ModelSpec,
        _context: &'a swink_agent::AgentContext,
        _options: &'a StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        let events = {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                vec![AssistantMessageEvent::Error {
                    stop_reason: StopReason::Error,
                    error_message: "no more scripted responses".to_string(),
                    usage: None,
                    error_kind: None,
                }]
            } else {
                responses.remove(0)
            }
        };
        Box::pin(futures::stream::iter(events))
    }
}

// ─── ContextCapturingStreamFn ─────────────────────────────────────────────

/// A mock `StreamFn` that captures the number of messages passed in each call.
#[allow(dead_code)]
pub struct ContextCapturingStreamFn {
    pub responses: Mutex<Vec<Vec<AssistantMessageEvent>>>,
    pub captured_message_counts: Mutex<Vec<usize>>,
}

#[allow(dead_code)]
impl ContextCapturingStreamFn {
    pub const fn new(responses: Vec<Vec<AssistantMessageEvent>>) -> Self {
        Self {
            responses: Mutex::new(responses),
            captured_message_counts: Mutex::new(Vec::new()),
        }
    }
}

impl StreamFn for ContextCapturingStreamFn {
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
        let events = {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                vec![AssistantMessageEvent::Error {
                    stop_reason: StopReason::Error,
                    error_message: "no more scripted responses".to_string(),
                    usage: None,
                    error_kind: None,
                }]
            } else {
                responses.remove(0)
            }
        };
        Box::pin(futures::stream::iter(events))
    }
}

// ─── ApiKeyCapturingStreamFn ──────────────────────────────────────────────

/// A mock `StreamFn` that captures resolved API keys from stream options.
#[allow(dead_code)]
pub struct ApiKeyCapturingStreamFn {
    pub responses: Mutex<Vec<Vec<AssistantMessageEvent>>>,
    pub captured_api_keys: Mutex<Vec<Option<String>>>,
}

#[allow(dead_code)]
impl ApiKeyCapturingStreamFn {
    pub const fn new(responses: Vec<Vec<AssistantMessageEvent>>) -> Self {
        Self {
            responses: Mutex::new(responses),
            captured_api_keys: Mutex::new(Vec::new()),
        }
    }
}

impl StreamFn for ApiKeyCapturingStreamFn {
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
        let events = {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                vec![AssistantMessageEvent::Error {
                    stop_reason: StopReason::Error,
                    error_message: "no more scripted responses".to_string(),
                    usage: None,
                    error_kind: None,
                }]
            } else {
                responses.remove(0)
            }
        };
        Box::pin(futures::stream::iter(events))
    }
}

// ─── MockTool ────────────────────────────────────────────────────────────

/// A configurable mock tool for testing.
#[allow(dead_code)]
pub struct MockTool {
    pub tool_name: String,
    pub schema: Value,
    pub result: Mutex<Option<AgentToolResult>>,
    pub delay: Option<Duration>,
    pub executed: AtomicBool,
    pub execute_count: AtomicU32,
    pub approval_required: bool,
}

#[allow(dead_code)]
impl MockTool {
    pub fn new(name: &str) -> Self {
        Self {
            tool_name: name.to_string(),
            schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": true
            }),
            result: Mutex::new(Some(AgentToolResult::text("ok"))),
            delay: None,
            executed: AtomicBool::new(false),
            execute_count: AtomicU32::new(0),
            approval_required: false,
        }
    }

    pub fn with_schema(mut self, schema: Value) -> Self {
        self.schema = schema;
        self
    }

    pub fn with_result(self, result: AgentToolResult) -> Self {
        *self.result.lock().unwrap() = Some(result);
        self
    }

    pub const fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }

    pub const fn with_requires_approval(mut self, required: bool) -> Self {
        self.approval_required = required;
        self
    }

    pub fn was_executed(&self) -> bool {
        self.executed.load(Ordering::SeqCst)
    }

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
            .unwrap_or_else(|| AgentToolResult::text("ok"));
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

// ─── Helper functions ────────────────────────────────────────────────────

/// Default model spec for tests.
#[allow(dead_code)]
pub fn default_model() -> ModelSpec {
    ModelSpec::new("test", "test-model")
}

/// Default message converter for tests.
#[allow(dead_code)]
pub fn default_convert(msg: &AgentMessage) -> Option<LlmMessage> {
    match msg {
        AgentMessage::Llm(llm) => Some(llm.clone()),
        AgentMessage::Custom(_) => None,
    }
}

/// Build a single user message.
#[allow(dead_code)]
pub fn user_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        timestamp: 0,
    }))
}

/// Build a sequence of events that produces a single text response.
#[allow(dead_code)]
pub fn text_only_events(text: &str) -> Vec<AssistantMessageEvent> {
    vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::TextStart { content_index: 0 },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: text.to_string(),
        },
        AssistantMessageEvent::TextEnd { content_index: 0 },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            usage: Usage::default(),
            cost: Cost::default(),
        },
    ]
}

/// Build events for a tool call response.
#[allow(dead_code)]
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
pub fn error_events(
    message: &str,
    error_kind: Option<swink_agent::StreamErrorKind>,
) -> Vec<AssistantMessageEvent> {
    vec![AssistantMessageEvent::Error {
        stop_reason: StopReason::Error,
        error_message: message.to_string(),
        usage: None,
        error_kind,
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
impl EventCollector {
    pub fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Returns a closure suitable for `agent.subscribe(...)`.
    pub fn subscriber(&self) -> impl Fn(&swink_agent::AgentEvent) + Send + Sync + 'static {
        let events = Arc::clone(&self.events);
        move |event: &swink_agent::AgentEvent| {
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
pub fn event_variant_name(event: &swink_agent::AgentEvent) -> String {
    use swink_agent::AgentEvent;
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
        _ => "Unknown".into(),
    }
}
