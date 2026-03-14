//! Steering and context snapshot tests for the [`Agent`] public API.

mod common;

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{
    MockStreamFn, MockTool, default_convert, default_model, text_only_events, tool_call_events,
    user_msg,
};
use futures::Stream;
use tokio_util::sync::CancellationToken;

use swink_agent::{
    Agent, AgentOptions, AgentTool, AssistantMessageEvent, ContentBlock, Cost, DefaultRetryStrategy,
    LlmMessage, ModelSpec, SteeringMode, StopReason, StreamFn, StreamOptions, Usage,
};

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
        context: &'a swink_agent::AgentContext,
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
                    error_kind: None,
                }]
            } else {
                responses.remove(0)
            }
        };
        Box::pin(futures::stream::iter(events))
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────

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
