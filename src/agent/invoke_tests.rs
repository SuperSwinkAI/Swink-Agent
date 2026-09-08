//! Tests for `invoke`.
#![cfg(test)]

use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use super::new_blocking_runtime_with;
use crate::Agent;
use crate::agent::AgentOptions;
use crate::error::AgentError;
use crate::policy::{PreDispatchPolicy, PreDispatchVerdict, ToolDispatchContext};
use crate::stream::{AssistantMessageEvent, StreamFn, StreamOptions};
use crate::tool::ApprovalMode;
use crate::tool::{AgentTool, AgentToolResult, ToolFuture, permissive_object_schema};
use crate::types::{
    AgentContext, AgentMessage, ContentBlock, Cost, LlmMessage, ModelSpec, StopReason, Usage,
    UserMessage,
};
use futures::Stream;
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

struct StopEveryToolPolicy;
struct CountingStreamFn {
    call_count: AtomicU32,
    responses: Mutex<Vec<Vec<AssistantMessageEvent>>>,
}
struct CountingTool {
    executions: Arc<AtomicU32>,
}
struct DroppableTool {
    executions: Arc<AtomicU32>,
    dropped: Arc<Notify>,
}
struct DropNotify {
    dropped: Arc<Notify>,
}

impl PreDispatchPolicy for StopEveryToolPolicy {
    fn name(&self) -> &'static str {
        "stop-every-tool"
    }

    fn evaluate(&self, _ctx: &mut ToolDispatchContext<'_>) -> PreDispatchVerdict {
        PreDispatchVerdict::Stop("blocked by test policy".to_string())
    }
}

impl CountingStreamFn {
    fn new(responses: Vec<Vec<AssistantMessageEvent>>) -> Self {
        Self {
            call_count: AtomicU32::new(0),
            responses: Mutex::new(responses),
        }
    }
}

impl StreamFn for CountingStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a ModelSpec,
        _context: &'a AgentContext,
        _options: &'a StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        let events = self.responses.lock().unwrap().pop().unwrap_or_else(|| {
            vec![AssistantMessageEvent::Error {
                stop_reason: StopReason::Error,
                error_message: "no more scripted responses".to_string(),
                usage: None,
                error_kind: None,
                retry_after: None,
            }]
        });
        Box::pin(futures::stream::iter(events))
    }
}

impl AgentTool for CountingTool {
    fn name(&self) -> &'static str {
        "tool_one"
    }

    fn label(&self) -> &'static str {
        "tool_one"
    }

    fn description(&self) -> &'static str {
        "test tool"
    }

    fn parameters_schema(&self) -> &Value {
        static SCHEMA: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
        SCHEMA.get_or_init(permissive_object_schema)
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        _params: Value,
        _cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: std::sync::Arc<std::sync::RwLock<crate::SessionState>>,
        _credential: Option<crate::credential::ResolvedCredential>,
    ) -> ToolFuture<'_> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { AgentToolResult::text("ok") })
    }
}

impl Drop for DropNotify {
    fn drop(&mut self) {
        self.dropped.notify_waiters();
    }
}

impl AgentTool for DroppableTool {
    fn name(&self) -> &'static str {
        "droppable_tool"
    }

    fn label(&self) -> &'static str {
        "droppable_tool"
    }

    fn description(&self) -> &'static str {
        "test tool"
    }

    fn parameters_schema(&self) -> &Value {
        static SCHEMA: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
        SCHEMA.get_or_init(permissive_object_schema)
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
        self.executions.fetch_add(1, Ordering::SeqCst);
        let guard = DropNotify {
            dropped: Arc::clone(&self.dropped),
        };
        Box::pin(async move {
            let _guard = guard;
            cancellation_token.cancelled().await;
            AgentToolResult::error("cancelled")
        })
    }
}

fn tool_call_events(id: &str, name: &str, args: &str) -> Vec<AssistantMessageEvent> {
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

fn user_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        timestamp: 0,
        cache_hint: None,
    }))
}

#[test]
fn new_blocking_runtime_returns_runtime_init_error() {
    let err = new_blocking_runtime_with(|| Err(std::io::Error::other("boom"))).unwrap_err();

    assert!(matches!(err, AgentError::RuntimeInit { .. }));
    assert_eq!(
        err.to_string(),
        "failed to create Tokio runtime for sync API"
    );
}

#[tokio::test]
async fn pre_dispatch_stop_does_not_trigger_a_follow_up_model_turn() {
    let stream_fn = Arc::new(CountingStreamFn::new(vec![tool_call_events(
        "call_1", "tool_one", "{}",
    )]));
    let executions = Arc::new(AtomicU32::new(0));
    let tool = Arc::new(CountingTool {
        executions: Arc::clone(&executions),
    });

    let options = AgentOptions::new(
        "sys",
        ModelSpec::new("test", "test-model"),
        Arc::clone(&stream_fn) as Arc<dyn StreamFn>,
        crate::agent::default_convert,
    )
    .with_tools(vec![tool as Arc<dyn crate::tool::AgentTool>])
    .with_approval_mode(ApprovalMode::Bypassed)
    .with_pre_dispatch_policy(StopEveryToolPolicy);
    let mut agent = Agent::new(options);

    let result = agent
        .prompt_async(vec![user_msg("run the tool")])
        .await
        .expect("prompt should complete");

    assert_eq!(
        stream_fn.call_count.load(Ordering::SeqCst),
        1,
        "a stopped pre-dispatch batch must terminate before the next LLM turn"
    );
    assert_eq!(
        executions.load(Ordering::SeqCst),
        0,
        "stop should prevent dispatch"
    );
    assert_eq!(
        result.messages.len(),
        2,
        "the run should end with the assistant tool call plus one synthetic result"
    );
    assert!(matches!(
        result.messages.as_slice(),
        [
            crate::types::AgentMessage::Llm(crate::types::LlmMessage::Assistant(message)),
            crate::types::AgentMessage::Llm(crate::types::LlmMessage::ToolResult(tool_result)),
        ]
            if matches!(message.content.as_slice(), [crate::types::ContentBlock::ToolCall { id, name, .. }] if id == "call_1" && name == "tool_one")
                && tool_result.tool_call_id == "call_1"
                && tool_result.is_error
    ));
}

#[tokio::test]
async fn dropping_prompt_stream_cancels_active_loop() {
    let stream_fn = Arc::new(CountingStreamFn::new(vec![
        AssistantMessageEvent::text_response("second"),
        tool_call_events("call_1", "droppable_tool", "{}"),
    ]));
    let executions = Arc::new(AtomicU32::new(0));
    let dropped = Arc::new(Notify::new());
    let tool = Arc::new(DroppableTool {
        executions: Arc::clone(&executions),
        dropped: Arc::clone(&dropped),
    });
    let options = AgentOptions::new(
        "sys",
        ModelSpec::new("test", "test-model"),
        Arc::clone(&stream_fn) as Arc<dyn StreamFn>,
        crate::agent::default_convert,
    )
    .with_tools(vec![tool as Arc<dyn crate::tool::AgentTool>])
    .with_approval_mode(ApprovalMode::Bypassed);
    let mut agent = Agent::new(options);

    let mut stream = agent
        .prompt_stream(vec![user_msg("run the tool")])
        .expect("prompt_stream should start");
    while let Some(event) = stream.next().await {
        if matches!(event, crate::loop_::AgentEvent::ToolExecutionStart { .. }) {
            break;
        }
    }
    let dropped_notified = dropped.notified();
    drop(stream);

    tokio::time::timeout(std::time::Duration::from_secs(1), dropped_notified)
        .await
        .expect("dropping the stream should tear down active tool work");

    let result = agent
        .prompt_async(vec![user_msg("run again")])
        .await
        .expect("a new run should be allowed after dropping the old stream");

    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(stream_fn.call_count.load(Ordering::SeqCst), 2);
    assert_eq!(result.stop_reason, StopReason::Stop);
}
