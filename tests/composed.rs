//! Composed integration tests that exercise multiple features together.
//!
//! These tests fill the gap where features are only tested in isolation,
//! verifying that approval, steering, follow-up, cancellation, overflow,
//! structured output, and event subscriptions compose correctly.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::Stream;
use futures::stream::StreamExt;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use swink_agent::{
    Agent, AgentEvent, AgentMessage, AgentOptions, AgentTool, AgentToolResult,
    AssistantMessageEvent, ContentBlock, Cost, DefaultRetryStrategy, LlmMessage, ModelSpec,
    StopReason, StreamFn, StreamOptions, ToolApproval, Usage, UserMessage, selective_approve,
};

// ─── MockStreamFn ────────────────────────────────────────────────────────

/// A mock `StreamFn` that yields scripted event sequences.
struct MockStreamFn {
    responses: Mutex<Vec<Vec<AssistantMessageEvent>>>,
}

impl MockStreamFn {
    const fn new(responses: Vec<Vec<AssistantMessageEvent>>) -> Self {
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
struct MockTool {
    tool_name: String,
    schema: Value,
    result: Mutex<Option<AgentToolResult>>,
    delay: Option<Duration>,
    executed: AtomicBool,
    execute_count: AtomicU32,
    approval_required: bool,
}

impl MockTool {
    fn new(name: &str) -> Self {
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

    fn with_result(self, result: AgentToolResult) -> Self {
        *self.result.lock().unwrap() = Some(result);
        self
    }

    const fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }

    const fn with_requires_approval(mut self, required: bool) -> Self {
        self.approval_required = required;
        self
    }

    fn was_executed(&self) -> bool {
        self.executed.load(Ordering::SeqCst)
    }

    fn execution_count(&self) -> u32 {
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
        _cancellation_token: CancellationToken,
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
                tokio::time::sleep(d).await;
            }
            result
        })
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────

fn default_model() -> ModelSpec {
    ModelSpec::new("test", "test-model")
}

fn user_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        timestamp: 0,
    }))
}

fn text_only_events(text: &str) -> Vec<AssistantMessageEvent> {
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

/// Build two tool calls in a single assistant turn.
fn two_tool_call_events(
    id1: &str,
    name1: &str,
    args1: &str,
    id2: &str,
    name2: &str,
    args2: &str,
) -> Vec<AssistantMessageEvent> {
    vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            id: id1.to_string(),
            name: name1.to_string(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            delta: args1.to_string(),
        },
        AssistantMessageEvent::ToolCallEnd { content_index: 0 },
        AssistantMessageEvent::ToolCallStart {
            content_index: 1,
            id: id2.to_string(),
            name: name2.to_string(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 1,
            delta: args2.to_string(),
        },
        AssistantMessageEvent::ToolCallEnd { content_index: 1 },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
            cost: Cost::default(),
        },
    ]
}

fn default_convert(msg: &AgentMessage) -> Option<LlmMessage> {
    match msg {
        AgentMessage::Llm(llm) => Some(llm.clone()),
        AgentMessage::Custom(_) => None,
    }
}

fn make_options(
    responses: Vec<Vec<AssistantMessageEvent>>,
    tools: Vec<Arc<dyn AgentTool>>,
) -> AgentOptions {
    AgentOptions::new(
        "test system prompt",
        default_model(),
        Arc::new(MockStreamFn::new(responses)),
        default_convert,
    )
    .with_tools(tools)
    .with_retry_strategy(Box::new(
        DefaultRetryStrategy::default()
            .with_jitter(false)
            .with_base_delay(Duration::from_millis(1)),
    ))
}

// ─── Test 1: Approval with steering interrupt ────────────────────────────

/// Start a run with tools requiring approval, approve one, then steer with
/// a new message mid-execution. Verify the steering message is processed
/// after the current tool batch completes.
#[tokio::test]
async fn approval_with_steering_interrupt() {
    let tool = Arc::new(
        MockTool::new("my_tool")
            .with_requires_approval(true)
            .with_delay(Duration::from_millis(30)),
    );
    let tool_ref = Arc::clone(&tool);

    // Turn 1: tool call (approved), Turn 2: response after steering consumed.
    let responses = vec![
        tool_call_events("tc1", "my_tool", "{}"),
        text_only_events("after steering"),
    ];

    let options = make_options(responses, vec![tool]).with_approve_tool(|_req| {
        Box::pin(async { ToolApproval::Approved })
    });
    let mut agent = Agent::new(options);

    // Queue a steering message before the run so it is consumed during execution.
    agent.steer(user_msg("change direction"));

    let result = agent.prompt_async(vec![user_msg("go")]).await.unwrap();

    assert!(tool_ref.was_executed(), "approved tool should execute");
    assert!(result.error.is_none(), "run should complete without error");

    // The steering message should have been consumed (no pending messages).
    assert!(
        !agent.has_pending_messages(),
        "steering message should be consumed during the run"
    );

    // The result should include messages from both turns.
    let has_post_steering = result.messages.iter().any(|m| {
        matches!(m, AgentMessage::Llm(LlmMessage::Assistant(a))
            if a.content.iter().any(|b| matches!(b, ContentBlock::Text { text } if text == "after steering")))
    });
    assert!(
        has_post_steering,
        "should contain the response after steering was processed"
    );
}

// ─── Test 2: Multi-tool selective approval ───────────────────────────────

/// Register multiple tools, some requiring approval and some not. Use
/// `selective_approve` to verify non-requiring tools execute immediately
/// while requiring ones go through the callback.
#[tokio::test]
async fn multi_tool_approval_selective() {
    let safe_tool = Arc::new(MockTool::new("safe_tool"));
    let dangerous_tool = Arc::new(MockTool::new("dangerous_tool").with_requires_approval(true));
    let safe_ref = Arc::clone(&safe_tool);
    let dangerous_ref = Arc::clone(&dangerous_tool);

    let inner_called = Arc::new(AtomicU32::new(0));
    let inner_flag = Arc::clone(&inner_called);

    // Turn 1: both tools called in one batch. Turn 2: text stop.
    let responses = vec![
        two_tool_call_events("tc1", "safe_tool", "{}", "tc2", "dangerous_tool", "{}"),
        text_only_events("done"),
    ];

    let options = make_options(responses, vec![safe_tool, dangerous_tool]).with_approve_tool(
        selective_approve(move |_req| {
            inner_flag.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { ToolApproval::Approved })
        }),
    );
    let mut agent = Agent::new(options);

    let result = agent.prompt_async(vec![user_msg("go")]).await.unwrap();

    assert!(result.error.is_none());
    assert!(
        safe_ref.was_executed(),
        "safe tool should execute without approval callback"
    );
    assert!(
        dangerous_ref.was_executed(),
        "dangerous tool should execute after approval"
    );

    // The inner callback should only be called for the dangerous tool.
    assert_eq!(
        inner_called.load(Ordering::SeqCst),
        1,
        "inner approval callback should be called exactly once (for the requiring tool)"
    );
}

// ─── Test 3: Follow-up after tool error ──────────────────────────────────

/// Tool execution returns an error result. Verify the error is sent back to
/// the LLM and a follow-up turn processes correctly.
#[tokio::test]
async fn follow_up_after_tool_error() {
    let error_tool = Arc::new(
        MockTool::new("failing_tool")
            .with_result(AgentToolResult::error("error: tool failed badly")),
    );

    // Turn 1: tool call returns error. Turn 2: LLM acknowledges error.
    // Turn 3: follow-up response.
    let responses = vec![
        tool_call_events("tc1", "failing_tool", "{}"),
        text_only_events("I see the tool failed"),
        text_only_events("follow-up answer"),
    ];

    let options = make_options(responses, vec![error_tool]);
    let mut agent = Agent::new(options);

    // Queue a follow-up so the loop continues after the error turn.
    agent.follow_up(user_msg("what happened?"));

    let result = agent.prompt_async(vec![user_msg("run the tool")]).await.unwrap();
    assert!(result.error.is_none(), "run should complete without error");

    // Verify the error was sent back to the LLM as a tool result.
    let has_error_result = result.messages.iter().any(|msg| {
        if let AgentMessage::Llm(LlmMessage::ToolResult(tr)) = msg {
            tr.content
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("tool failed badly")))
        } else {
            false
        }
    });
    assert!(
        has_error_result,
        "tool error should appear as a tool result in messages"
    );

    // Verify the follow-up was consumed.
    assert!(
        !agent.has_pending_messages(),
        "follow-up should be consumed"
    );

    // Verify the final response came through.
    let has_follow_up_response = result.messages.iter().any(|m| {
        matches!(m, AgentMessage::Llm(LlmMessage::Assistant(a))
            if a.content.iter().any(|b| matches!(b, ContentBlock::Text { text } if text == "follow-up answer")))
    });
    assert!(
        has_follow_up_response,
        "should contain the follow-up response"
    );
}

// ─── Test 4: Abort during tool execution with approval ───────────────────

/// Start a run with approved tool calls in flight, then abort. Verify
/// cancellation propagates and the agent stops cleanly.
#[tokio::test]
async fn abort_during_tool_execution_with_approval() {
    let tool = Arc::new(
        MockTool::new("slow_approved")
            .with_requires_approval(true)
            .with_delay(Duration::from_secs(10)),
    );

    let responses = vec![
        tool_call_events("tc1", "slow_approved", "{}"),
        text_only_events("should not reach"),
    ];

    let options = make_options(responses, vec![tool]).with_approve_tool(|_req| {
        Box::pin(async { ToolApproval::Approved })
    });
    let mut agent = Agent::new(options);

    let mut stream = agent.prompt_stream(vec![user_msg("go")]).unwrap();

    // Consume events until we see tool execution start, then abort.
    let mut saw_tool_start = false;
    let mut saw_approval_requested = false;
    while let Some(event) = stream.next().await {
        if matches!(event, AgentEvent::ToolApprovalRequested { .. }) {
            saw_approval_requested = true;
        }
        if matches!(event, AgentEvent::ToolExecutionStart { .. }) {
            saw_tool_start = true;
            agent.abort();
        }
        // Once aborted the stream will end.
    }

    assert!(
        saw_approval_requested,
        "should see approval requested event"
    );
    assert!(saw_tool_start, "should see tool execution start");
    // The stream ended (we exited the while loop), meaning the abort
    // propagated and the agent loop terminated cleanly.
}

// ─── Test 5: Context overflow triggers retry with tools ──────────────────

/// Set up a scenario where the context overflows during a tool-use turn,
/// triggering the overflow retry path. Verify the tool is re-executed on
/// the retry turn.
#[tokio::test]
async fn context_overflow_triggers_retry_with_tools() {
    let tool = Arc::new(MockTool::new("my_tool"));
    let tool_ref = Arc::clone(&tool);

    let overflow_flags: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));
    let flags_clone = Arc::clone(&overflow_flags);

    // Turn 1: tool call. Turn 2: overflow error (simulating context window exceeded).
    // Turn 3 (retry after overflow): tool call again. Turn 4: final text.
    let responses = vec![
        tool_call_events("tc1", "my_tool", "{}"),
        vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::Error {
                stop_reason: StopReason::Error,
                error_message: "context window exceeded".to_string(),
                usage: None,
            },
        ],
        tool_call_events("tc2", "my_tool", "{}"),
        text_only_events("recovered after overflow"),
    ];

    let options = make_options(responses, vec![tool]).with_transform_context(
        move |_msgs: &mut Vec<AgentMessage>, overflow: bool| {
            flags_clone.lock().unwrap().push(overflow);
        },
    );
    let mut agent = Agent::new(options);

    let result = agent.prompt_async(vec![user_msg("go")]).await.unwrap();
    assert!(result.error.is_none(), "run should complete without error");

    // The tool should have been executed at least twice (once per tool-use turn).
    assert!(
        tool_ref.execution_count() >= 2,
        "tool should execute at least twice (original + retry), got {}",
        tool_ref.execution_count()
    );

    // The overflow flag should have been set to true on at least one
    // `transform_context` call.
    assert!(
        overflow_flags.lock().unwrap().iter().any(|&f| f),
        "transform_context should receive overflow=true after context window exceeded"
    );
}

// ─── Test 6: Structured output with tool calls ──────────────────────────

/// Model returns a `__structured_output` tool call with valid arguments.
/// Verify structured output works alongside the normal tool execution path.
#[tokio::test]
async fn structured_output_with_tool_calls() {
    let schema = json!({
        "type": "object",
        "properties": {
            "answer": { "type": "string" },
            "confidence": { "type": "number" }
        },
        "required": ["answer", "confidence"]
    });

    // The LLM calls __structured_output with valid arguments. After tool
    // execution the loop calls the LLM again, which returns text to end.
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events(
            "so_1",
            "__structured_output",
            r#"{"answer": "42", "confidence": 0.95}"#,
        ),
        text_only_events("done"),
    ]));

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_retry_strategy(Box::new(
                DefaultRetryStrategy::default()
                    .with_jitter(false)
                    .with_base_delay(Duration::from_millis(1)),
            )),
    );

    let value = agent
        .structured_output("What is the answer?".to_string(), schema)
        .await
        .unwrap();

    assert_eq!(value["answer"], "42");
    assert_eq!(value["confidence"], 0.95);
}

// ─── Test 7: Subscriber receives approval events ─────────────────────────

/// Subscribe to events, trigger tool approval, verify the subscriber
/// receives approval-related events in the correct order alongside
/// standard lifecycle events.
#[tokio::test]
async fn subscriber_receives_approval_events() {
    let tool = Arc::new(MockTool::new("guarded_tool").with_requires_approval(true));
    let events_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let responses = vec![
        tool_call_events("tc1", "guarded_tool", "{}"),
        text_only_events("done"),
    ];

    let options = make_options(responses, vec![tool]).with_approve_tool(|_req| {
        Box::pin(async { ToolApproval::Approved })
    });
    let mut agent = Agent::new(options);

    let log = Arc::clone(&events_log);
    agent.subscribe(move |event| {
        let name = match event {
            AgentEvent::AgentStart => "AgentStart",
            AgentEvent::TurnStart => "TurnStart",
            AgentEvent::MessageStart => "MessageStart",
            AgentEvent::MessageEnd { .. } => "MessageEnd",
            AgentEvent::ToolExecutionStart { .. } => "ToolExecutionStart",
            AgentEvent::ToolApprovalRequested { .. } => "ToolApprovalRequested",
            AgentEvent::ToolApprovalResolved { .. } => "ToolApprovalResolved",
            AgentEvent::ToolExecutionEnd { .. } => "ToolExecutionEnd",
            AgentEvent::TurnEnd { .. } => "TurnEnd",
            AgentEvent::AgentEnd { .. } => "AgentEnd",
            _ => return,
        };
        log.lock().unwrap().push(name.to_string());
    });

    let result = agent.prompt_async(vec![user_msg("go")]).await.unwrap();
    assert!(result.error.is_none());

    let events: Vec<String> = events_log.lock().unwrap().clone();

    // Verify the overall lifecycle ordering.
    let find = |name: &str| events.iter().position(|n| n == name);
    let agent_start = find("AgentStart").expect("should have AgentStart");
    let turn_start = find("TurnStart").expect("should have TurnStart");
    let tool_start = find("ToolExecutionStart").expect("should have ToolExecutionStart");
    let approval_requested = find("ToolApprovalRequested").expect("should have ToolApprovalRequested");
    let approval_resolved = find("ToolApprovalResolved").expect("should have ToolApprovalResolved");
    let tool_end = find("ToolExecutionEnd").expect("should have ToolExecutionEnd");
    let agent_end = find("AgentEnd").expect("should have AgentEnd");

    assert!(agent_start < turn_start, "AgentStart before TurnStart");
    assert!(
        turn_start < tool_start,
        "TurnStart before ToolExecutionStart"
    );
    assert!(
        tool_start < approval_requested,
        "ToolExecutionStart before ApprovalRequested"
    );
    assert!(
        approval_requested < approval_resolved,
        "ApprovalRequested before ApprovalResolved"
    );
    assert!(
        approval_resolved < tool_end,
        "ApprovalResolved before ToolExecutionEnd"
    );
    assert!(tool_end < agent_end, "ToolExecutionEnd before AgentEnd");
}
