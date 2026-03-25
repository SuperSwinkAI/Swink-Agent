//! Shared test mocks for policy integration tests.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use futures::Stream;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use swink_agent::{
    AgentTool, AgentToolResult, AssistantMessageEvent, Cost, ModelSpec, StopReason, StreamFn,
    StreamOptions, Usage,
};

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
            let mut guard = self.responses.lock().unwrap();
            if guard.is_empty() {
                vec![AssistantMessageEvent::Error {
                    stop_reason: StopReason::Error,
                    error_message: "no more scripted responses".to_string(),
                    usage: None,
                    error_kind: None,
                }]
            } else {
                guard.remove(0)
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
        }
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
        false
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
