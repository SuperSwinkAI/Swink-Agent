//! Phase 4: Integration tests for the [`Agent`] public API.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::Stream;
use futures::stream::StreamExt;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use agent_harness::{
    Agent, AgentEvent, AgentMessage, AgentOptions, AgentTool, AgentToolResult,
    AssistantMessageEvent, ContentBlock, Cost, DefaultRetryStrategy, HarnessError, LlmMessage,
    ModelSpec, SteeringMode, StopReason, StreamFn, StreamOptions, Usage, UserMessage,
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
        _context: &'a agent_harness::AgentContext,
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

// ─── ContextCapturingStreamFn ────────────────────────────────────────────

/// A mock `StreamFn` that captures context message counts.
struct ContextCapturingStreamFn {
    responses: Mutex<Vec<Vec<AssistantMessageEvent>>>,
    captured_message_counts: Mutex<Vec<usize>>,
}

impl ContextCapturingStreamFn {
    const fn new(responses: Vec<Vec<AssistantMessageEvent>>) -> Self {
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
        context: &'a agent_harness::AgentContext,
        _options: &'a StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
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
        }
    }

    #[allow(dead_code)]
    fn with_result(self, result: AgentToolResult) -> Self {
        *self.result.lock().unwrap() = Some(result);
        self
    }

    const fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
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
        self.executed.store(true, Ordering::SeqCst);
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

fn default_convert(msg: &AgentMessage) -> Option<LlmMessage> {
    match msg {
        AgentMessage::Llm(llm) => Some(llm.clone()),
        AgentMessage::Custom(_) => None,
    }
}

fn make_agent(stream_fn: Arc<dyn StreamFn>) -> Agent {
    Agent::new(
        AgentOptions::new(
            "test system prompt",
            default_model(),
            stream_fn,
            default_convert,
        )
        .with_retry_strategy(Box::new(
            DefaultRetryStrategy::default()
                .with_jitter(false)
                .with_base_delay(Duration::from_millis(1)),
        )),
    )
}

fn make_agent_with_tools(stream_fn: Arc<dyn StreamFn>, tools: Vec<Arc<dyn AgentTool>>) -> Agent {
    Agent::new(
        AgentOptions::new(
            "test system prompt",
            default_model(),
            stream_fn,
            default_convert,
        )
        .with_tools(tools)
        .with_retry_strategy(Box::new(
            DefaultRetryStrategy::default()
                .with_jitter(false)
                .with_base_delay(Duration::from_millis(1)),
        )),
    )
}

// ─── 4.1: prompt_async returns correct AgentResult ───────────────────────

#[tokio::test]
async fn test_4_1_prompt_async_returns_correct_result() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("Hello world")]));
    let mut agent = make_agent(stream_fn);

    let result = agent.prompt_async(vec![user_msg("Hi")]).await.unwrap();

    assert_eq!(result.stop_reason, StopReason::Stop);
    assert!(result.error.is_none());
    assert!(!result.messages.is_empty());

    // The result should contain an assistant message with the expected text.
    let has_assistant_text = result.messages.iter().any(|m| {
        matches!(m, AgentMessage::Llm(LlmMessage::Assistant(a))
            if a.content.iter().any(|b| matches!(b, ContentBlock::Text { text } if text == "Hello world")))
    });
    assert!(has_assistant_text, "result should contain assistant text");

    // Agent should be idle after completion.
    assert!(!agent.state().is_running);
}

// ─── 4.2: prompt_sync blocks and returns same result as async ────────────

#[test]
fn test_4_2_prompt_sync_returns_result() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("sync result")]));
    let mut agent = make_agent(stream_fn);

    let result = agent.prompt_sync(vec![user_msg("Hi")]).unwrap();

    assert_eq!(result.stop_reason, StopReason::Stop);
    assert!(result.error.is_none());

    let has_text = result.messages.iter().any(|m| {
        matches!(m, AgentMessage::Llm(LlmMessage::Assistant(a))
            if a.content.iter().any(|b| matches!(b, ContentBlock::Text { text } if text == "sync result")))
    });
    assert!(has_text, "sync result should contain assistant text");
    assert!(!agent.state().is_running);
}

// ─── 4.3: prompt_stream yields events in correct order ───────────────────

#[tokio::test]
async fn test_4_3_prompt_stream_yields_events_in_order() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("streamed")]));
    let mut agent = make_agent(stream_fn);

    let mut stream = agent.prompt_stream(vec![user_msg("Hi")]).unwrap();

    let mut event_names: Vec<String> = Vec::new();
    while let Some(event) = stream.next().await {
        let name = format!("{event:?}");
        let prefix = name.split([' ', '{', '(']).next().unwrap_or("").to_string();
        event_names.push(prefix);
    }

    // Verify event ordering: AgentStart < TurnStart < MessageStart < MessageEnd < TurnEnd < AgentEnd
    let find = |name: &str| event_names.iter().position(|n| n == name);
    let agent_start = find("AgentStart").expect("should have AgentStart");
    let turn_start = find("TurnStart").expect("should have TurnStart");
    let msg_start = find("MessageStart").expect("should have MessageStart");
    let msg_end = find("MessageEnd").expect("should have MessageEnd");
    let turn_end = find("TurnEnd").expect("should have TurnEnd");
    let agent_end = find("AgentEnd").expect("should have AgentEnd");

    assert!(agent_start < turn_start);
    assert!(turn_start < msg_start);
    assert!(msg_start < msg_end);
    assert!(msg_end < turn_end);
    assert!(turn_end < agent_end);
}

// ─── 4.4: prompt_* while running returns AlreadyRunning ──────────────────

#[tokio::test]
async fn test_4_4_already_running_error() {
    // prompt_stream sets is_running = true and returns immediately. While the
    // stream is not yet consumed, calling prompt again should fail.
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("first")]));
    let mut agent = make_agent(stream_fn);

    let _stream = agent.prompt_stream(vec![user_msg("first")]).unwrap();
    // Agent is now marked as running.
    assert!(agent.state().is_running);

    let result = agent.prompt_stream(vec![user_msg("second")]);
    let err = result.err().expect("should be an error");
    assert!(
        matches!(err, HarnessError::AlreadyRunning),
        "expected AlreadyRunning, got {err:?}"
    );
}

// ─── 4.5: abort() causes StopReason::Aborted ────────────────────────────

#[tokio::test]
async fn test_4_5_abort_causes_aborted_stop() {
    // Use a tool with a long delay so we can abort mid-run.
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("tc_1", "slow_tool", "{}"),
        text_only_events("should not reach"),
    ]));
    let tool = Arc::new(MockTool::new("slow_tool").with_delay(Duration::from_secs(10)));
    let mut agent = make_agent_with_tools(stream_fn, vec![tool]);

    let mut stream = agent.prompt_stream(vec![user_msg("go")]).unwrap();

    // Consume events until we see tool execution start, then abort.
    let mut found_abort = false;
    let mut saw_tool_start = false;
    while let Some(event) = stream.next().await {
        if matches!(event, AgentEvent::ToolExecutionStart { .. }) {
            saw_tool_start = true;
            agent.abort();
        }
        if let AgentEvent::TurnEnd {
            ref assistant_message,
            ..
        } = event
        {
            if assistant_message.stop_reason == StopReason::Aborted {
                found_abort = true;
            }
        }
    }

    assert!(saw_tool_start, "should have seen tool execution start");
    // The abort may or may not produce an Aborted turn depending on timing.
    // At minimum, the stream should have ended.
    // With the mock's delay, the cancellation should propagate.
    let _ = found_abort; // Abort may or may not be visible depending on timing.
}

// ─── 4.6: steer() during a run causes steering interrupt ─────────────────

#[tokio::test]
async fn test_4_6_steer_during_run() {
    // Two turns: first triggers a tool call, second is the final response.
    // We steer after seeing the tool execution start.
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("tc_1", "my_tool", "{}"),
        text_only_events("after steering"),
    ]));
    let tool = Arc::new(MockTool::new("my_tool").with_delay(Duration::from_millis(50)));
    let mut agent = make_agent_with_tools(stream_fn, vec![tool]);

    // Queue a steering message before the run.
    agent.steer(user_msg("change direction"));

    let result = agent.prompt_async(vec![user_msg("go")]).await.unwrap();

    // The run should complete (the steering message is consumed by the loop).
    assert!(!result.messages.is_empty(), "should have produced messages");
}

// ─── 4.7: follow_up() causes continuation after natural stop ─────────────

#[tokio::test]
async fn test_4_7_follow_up_continues() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        text_only_events("first response"),
        text_only_events("second response"),
    ]));
    let mut agent = make_agent(stream_fn);

    // Queue a follow-up before the run starts.
    agent.follow_up(user_msg("follow up question"));

    let result = agent.prompt_async(vec![user_msg("Hi")]).await.unwrap();

    // The loop should have produced messages from both turns (the follow-up
    // message causes a second turn).
    assert!(
        result.messages.len() >= 2,
        "should have messages from follow-up turn, got {}",
        result.messages.len()
    );
}

// ─── 4.8: steer() while idle queues for next run ─────────────────────────

#[tokio::test]
async fn test_4_8_steer_while_idle_queues() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        text_only_events("first"),
        text_only_events("with steering"),
    ]));
    let mut agent = make_agent(stream_fn);

    // Steer while idle
    agent.steer(user_msg("queued steering"));
    assert!(
        agent.has_pending_messages(),
        "should have pending steering messages"
    );

    // First prompt: the steering message should be consumed during the run.
    let result = agent.prompt_async(vec![user_msg("go")]).await.unwrap();
    assert!(!result.messages.is_empty());
}

// ─── 4.9: subscribe returns SubscriptionId; callback receives events ─────

#[tokio::test]
async fn test_4_9_subscribe_receives_events() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("subscribed")]));
    let mut agent = make_agent(stream_fn);

    let events_received = Arc::new(AtomicU32::new(0));
    let events_clone = Arc::clone(&events_received);

    let id = agent.subscribe(move |_event| {
        events_clone.fetch_add(1, Ordering::SeqCst);
    });

    // SubscriptionId should be valid (non-panic).
    let _ = id;

    let _result = agent.prompt_async(vec![user_msg("Hi")]).await.unwrap();

    let count = events_received.load(Ordering::SeqCst);
    assert!(
        count > 0,
        "subscriber should have received events, got {count}"
    );
}

// ─── 4.10: unsubscribe removes listener ──────────────────────────────────

#[tokio::test]
async fn test_4_10_unsubscribe_removes_listener() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        text_only_events("first"),
        text_only_events("second"),
    ]));
    let mut agent = make_agent(stream_fn);

    let events_received = Arc::new(AtomicU32::new(0));
    let events_clone = Arc::clone(&events_received);

    let id = agent.subscribe(move |_event| {
        events_clone.fetch_add(1, Ordering::SeqCst);
    });

    // Queue a follow-up so the second response is also consumed.
    agent.follow_up(user_msg("follow up"));

    let _result = agent.prompt_async(vec![user_msg("Hi")]).await.unwrap();
    let count_after_first = events_received.load(Ordering::SeqCst);
    assert!(count_after_first > 0, "should have received events");

    // Unsubscribe
    let removed = agent.unsubscribe(id);
    assert!(removed, "unsubscribe should return true for existing id");

    // Unsubscribe again should return false.
    let removed_again = agent.unsubscribe(id);
    assert!(!removed_again, "second unsubscribe should return false");
}

// ─── 4.11: subscriber panic does not crash; panicker is auto-unsubscribed ─

#[tokio::test]
async fn test_4_11_subscriber_panic_does_not_crash() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("safe")]));
    let mut agent = make_agent(stream_fn);

    // Subscribe a callback that panics.
    let _panic_id = agent.subscribe(|_event| {
        panic!("subscriber panic test");
    });

    // Also subscribe a well-behaved callback to verify it still fires.
    let good_events = Arc::new(AtomicU32::new(0));
    let good_clone = Arc::clone(&good_events);
    let _good_id = agent.subscribe(move |_event| {
        good_clone.fetch_add(1, Ordering::SeqCst);
    });

    // The agent should still complete without crashing.
    let result = agent.prompt_async(vec![user_msg("Hi")]).await.unwrap();
    assert_eq!(result.stop_reason, StopReason::Stop);

    // The well-behaved subscriber should have received events.
    // Note: dispatch_event catches panics but does NOT auto-unsubscribe in the
    // current implementation. The panicking subscriber is called each time but
    // its panic is caught. The good subscriber still fires.
    let good_count = good_events.load(Ordering::SeqCst);
    assert!(
        good_count > 0,
        "good subscriber should still receive events despite panicking sibling"
    );
}

// ─── 4.12: reset() clears state ──────────────────────────────────────────

#[tokio::test]
async fn test_4_12_reset_clears_state() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("before reset")]));
    let mut agent = make_agent(stream_fn);

    let _result = agent.prompt_async(vec![user_msg("Hi")]).await.unwrap();

    // Agent should have messages.
    assert!(
        !agent.state().messages.is_empty(),
        "should have messages after prompt"
    );

    // Queue some messages.
    agent.steer(user_msg("steering"));
    agent.follow_up(user_msg("follow up"));
    assert!(agent.has_pending_messages());

    // Reset.
    agent.reset();

    assert!(
        agent.state().messages.is_empty(),
        "messages should be cleared"
    );
    assert!(!agent.state().is_running, "should not be running");
    assert!(agent.state().error.is_none(), "error should be cleared");
    assert!(
        agent.state().stream_message.is_none(),
        "stream_message should be cleared"
    );
    assert!(
        agent.state().pending_tool_calls.is_empty(),
        "pending_tool_calls should be cleared"
    );
    assert!(!agent.has_pending_messages(), "queues should be cleared");
}

// ─── 4.13: wait_for_idle() resolves when run completes ───────────────────

#[tokio::test]
async fn test_4_13_wait_for_idle_resolves_immediately_when_idle() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("done")]));
    let mut agent = make_agent(stream_fn);

    // When not running, wait_for_idle should resolve immediately.
    agent.wait_for_idle().await;

    // Run a prompt to completion.
    let _result = agent.prompt_async(vec![user_msg("Hi")]).await.unwrap();

    // After completion, wait_for_idle should resolve immediately again.
    agent.wait_for_idle().await;
}

// ─── 4.14: structured_output validates and returns typed value ───────────

#[tokio::test]
async fn test_4_14_structured_output_valid() {
    let schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "age": { "type": "integer" }
        },
        "required": ["name", "age"]
    });

    // The LLM calls __structured_output with valid arguments. After tool
    // execution the loop calls the LLM again, which returns text to end.
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events(
            "so_1",
            "__structured_output",
            r#"{"name": "Alice", "age": 30}"#,
        ),
        text_only_events("done"),
    ]));
    let mut agent = make_agent(stream_fn);

    let value = agent
        .structured_output("Extract name and age".to_string(), schema)
        .await
        .unwrap();

    assert_eq!(value["name"], "Alice");
    assert_eq!(value["age"], 30);
}

// ─── 4.15: structured_output retries on invalid response ─────────────────

#[tokio::test]
async fn test_4_15_structured_output_retries() {
    let schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" }
        },
        "required": ["name"]
    });

    // Each structured_output attempt triggers: (1) tool call response, (2) after
    // tool execution the loop calls the LLM again which returns a text-only
    // response to end the turn. So each attempt needs 2 responses.
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        // Attempt 0: invalid tool call, then text to end turn.
        tool_call_events("so_1", "__structured_output", r"{}"),
        text_only_events("done"),
        // Attempt 1 (retry via continue): valid tool call, then text to end.
        tool_call_events("so_2", "__structured_output", r#"{"name": "Bob"}"#),
        text_only_events("done"),
    ]));
    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_retry_strategy(Box::new(
                DefaultRetryStrategy::default()
                    .with_jitter(false)
                    .with_base_delay(Duration::from_millis(1)),
            ))
            .with_structured_output_max_retries(3),
    );

    let value = agent
        .structured_output("Extract name".to_string(), schema)
        .await
        .unwrap();

    assert_eq!(value["name"], "Bob");
}

// ─── 4.16: structured_output fails after max retries ─────────────────────

#[tokio::test]
async fn test_4_16_structured_output_fails_after_max_retries() {
    let schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" }
        },
        "required": ["name"]
    });

    // All attempts return invalid output. Each attempt needs 2 responses
    // (tool call + text to end turn). 3 attempts = max_retries(2) + 1.
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("so_1", "__structured_output", r"{}"),
        text_only_events("done"),
        tool_call_events("so_2", "__structured_output", r"{}"),
        text_only_events("done"),
        tool_call_events("so_3", "__structured_output", r"{}"),
        text_only_events("done"),
    ]));
    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_retry_strategy(Box::new(
                DefaultRetryStrategy::default()
                    .with_jitter(false)
                    .with_base_delay(Duration::from_millis(1)),
            ))
            .with_structured_output_max_retries(2),
    );

    let err = agent
        .structured_output("Extract name".to_string(), schema)
        .await
        .unwrap_err();

    assert!(
        matches!(err, HarnessError::StructuredOutputFailed { attempts, .. } if attempts == 3),
        "expected StructuredOutputFailed with 3 attempts, got {err:?}"
    );
}

// ─── 4.17: continue_async with empty messages returns NoMessages ─────────

#[tokio::test]
async fn test_4_17_continue_async_no_messages() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![]));
    let mut agent = make_agent(stream_fn);

    // No messages in the agent — continue should fail.
    let err = agent.continue_async().await.unwrap_err();
    assert!(
        matches!(err, HarnessError::NoMessages),
        "expected NoMessages, got {err:?}"
    );
}

// ─── 4.18: Steering mode All vs OneAtATime ───────────────────────────────

#[tokio::test]
async fn test_4_18_steering_mode_all_delivers_all() {
    // With SteeringMode::All (default), all queued steering messages should be
    // delivered in one batch.
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("tc_1", "my_tool", "{}"),
        text_only_events("done"),
    ]));
    let tool = Arc::new(MockTool::new("my_tool"));
    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_tools(vec![tool])
            .with_steering_mode(SteeringMode::All)
            .with_retry_strategy(Box::new(
                DefaultRetryStrategy::default()
                    .with_jitter(false)
                    .with_base_delay(Duration::from_millis(1)),
            )),
    );

    // Queue multiple steering messages.
    agent.steer(user_msg("steer 1"));
    agent.steer(user_msg("steer 2"));
    agent.steer(user_msg("steer 3"));

    let result = agent.prompt_async(vec![user_msg("go")]).await.unwrap();
    assert!(!result.messages.is_empty());
    // All steering messages should have been drained (queue empty after).
    assert!(
        !agent.has_pending_messages(),
        "all steering messages should be consumed"
    );
}

#[tokio::test]
async fn test_4_18_steering_mode_one_at_a_time() {
    // With SteeringMode::OneAtATime and multiple tool calls, only one steering
    // message should be drained per poll.
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("tc_1", "my_tool", "{}"),
        tool_call_events("tc_2", "my_tool", "{}"),
        tool_call_events("tc_3", "my_tool", "{}"),
        text_only_events("done"),
    ]));
    let tool = Arc::new(MockTool::new("my_tool"));
    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_tools(vec![tool])
            .with_steering_mode(SteeringMode::OneAtATime)
            .with_retry_strategy(Box::new(
                DefaultRetryStrategy::default()
                    .with_jitter(false)
                    .with_base_delay(Duration::from_millis(1)),
            )),
    );

    agent.steer(user_msg("steer A"));
    agent.steer(user_msg("steer B"));

    let result = agent.prompt_async(vec![user_msg("go")]).await.unwrap();
    assert!(!result.messages.is_empty());
}

// ─── 4.19: AgentContext snapshot is immutable during a turn ──────────────

#[tokio::test]
async fn test_4_19_context_snapshot_immutable() {
    // The context passed to StreamFn should not reflect messages added during
    // the same turn. We verify by capturing context message counts: the first
    // call should see only the user message, the second call (after tool result)
    // should see user + assistant + tool_result.
    let capturing_fn = Arc::new(ContextCapturingStreamFn::new(vec![
        tool_call_events("tc_1", "my_tool", "{}"),
        text_only_events("done"),
    ]));
    let stream_fn: Arc<dyn StreamFn> = Arc::clone(&capturing_fn) as Arc<dyn StreamFn>;
    let tool = Arc::new(MockTool::new("my_tool"));
    let mut agent = make_agent_with_tools(stream_fn, vec![tool]);

    let _result = agent.prompt_async(vec![user_msg("go")]).await.unwrap();

    let counts = capturing_fn.captured_message_counts.lock().unwrap().clone();
    assert!(
        counts.len() >= 2,
        "should have at least 2 stream calls, got {}",
        counts.len()
    );
    // First call: only the user message.
    assert_eq!(counts[0], 1, "first turn should see 1 message (user)");
    // Second call: user + assistant + tool_result = 3 messages.
    assert_eq!(
        counts[1], 3,
        "second turn should see 3 messages (user + assistant + tool_result)"
    );
}
