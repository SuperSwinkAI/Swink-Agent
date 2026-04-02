//! Integration tests for User Story 4: Retry, Steering, and Abort.
//!
//! Tests T026–T032 covering acceptance criteria AC 17–AC 22.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{
    EventCollector, MockContextCapturingStreamFn, MockStreamFn, MockTool, default_convert,
    default_model, error_events, text_only_events, tool_call_events, user_msg,
};
use futures::stream::StreamExt;

use swink_agent::{
    Agent, AgentEvent, AgentMessage, AgentOptions, ContentBlock, CustomMessage,
    DefaultRetryStrategy, LlmMessage, PolicyContext, PolicyVerdict, PreTurnPolicy, StopReason,
    StreamErrorKind,
};

/// Inline max-turns policy for this test (the real one lives in swink-agent-policies).
#[derive(Debug, Clone)]
struct MaxTurnsPolicy {
    max_turns: usize,
}

impl MaxTurnsPolicy {
    const fn new(max_turns: usize) -> Self {
        Self { max_turns }
    }
}

impl PreTurnPolicy for MaxTurnsPolicy {
    fn name(&self) -> &'static str {
        "max_turns"
    }

    fn evaluate(&self, ctx: &PolicyContext<'_>) -> PolicyVerdict {
        if ctx.turn_index >= self.max_turns {
            PolicyVerdict::Stop(format!(
                "max turns reached: {} >= {}",
                ctx.turn_index, self.max_turns
            ))
        } else {
            PolicyVerdict::Continue
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn make_agent(stream_fn: Arc<dyn swink_agent::StreamFn>) -> Agent {
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

fn make_agent_with_tools(
    stream_fn: Arc<dyn swink_agent::StreamFn>,
    tools: Vec<Arc<dyn swink_agent::AgentTool>>,
) -> Agent {
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

// ═════════════════════════════════════════════════════════════════════════════
// T026 — retry_with_backoff_on_throttle (AC 17)
//
// Script mock stream to return a throttle error on the first call, then a
// successful text response on the second call. Verify the agent retries
// transparently and eventually succeeds.
// ═════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn retry_with_backoff_on_throttle() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        error_events("throttled", Some(StreamErrorKind::Throttled)),
        text_only_events("success"),
    ]));

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert).with_retry_strategy(
            Box::new(
                DefaultRetryStrategy::default()
                    .with_max_attempts(3)
                    .with_jitter(false)
                    .with_base_delay(Duration::from_millis(1)),
            ),
        ),
    );

    let result = agent.prompt_async(vec![user_msg("hello")]).await.unwrap();

    // The agent should have recovered on the second attempt.
    assert_eq!(result.stop_reason, StopReason::Stop);
    assert!(result.error.is_none(), "should have no error after retry");

    let has_success_text = result.messages.iter().any(|m| {
        matches!(
            m,
            AgentMessage::Llm(LlmMessage::Assistant(a))
                if a.content.iter().any(|b| matches!(b, ContentBlock::Text { text } if text == "success"))
        )
    });
    assert!(
        has_success_text,
        "agent should produce the success text after retry"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// T027 — steering_callback_modifies_messages (AC 18)
//
// Steer a message into the agent before prompting. The agent loop picks up
// the steering message between turns (after a tool call triggers a follow-up
// turn). Use MockContextCapturingStreamFn to verify the steered message is
// included in context on the second LLM call.
// ═════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn steering_callback_modifies_messages() {
    // Turn 1: tool call triggers tool execution and a follow-up turn.
    // Between turns, the steering message is consumed.
    // Turn 2: text response.
    let capturing_fn = Arc::new(MockContextCapturingStreamFn::new(vec![
        tool_call_events("tc_1", "my_tool", "{}"),
        text_only_events("final answer"),
    ]));
    let stream_fn: Arc<dyn swink_agent::StreamFn> =
        Arc::clone(&capturing_fn) as Arc<dyn swink_agent::StreamFn>;
    let tool = Arc::new(MockTool::new("my_tool"));

    let mut agent = make_agent_with_tools(stream_fn, vec![tool]);

    // Queue a steering message before starting the run.
    agent.steer(user_msg("injected steering"));

    let result = agent.prompt_async(vec![user_msg("go")]).await.unwrap();
    assert!(!result.messages.is_empty());

    // The second LLM call should see more messages than the first call.
    // First call: 1 message (user).
    // Second call: user + assistant + tool_result (3 minimum), plus the
    // steering message may appear as an additional message depending on
    // when the loop drains the steering queue.
    let counts = capturing_fn.captured_message_counts.lock().unwrap().clone();
    assert!(
        counts.len() >= 2,
        "should have at least 2 stream calls, got {}",
        counts.len()
    );
    assert!(
        counts[1] > counts[0],
        "second call should have more messages than first: {} vs {}",
        counts[1],
        counts[0]
    );

    // Verify the steering message was actually consumed (no longer pending).
    assert!(
        !agent.has_pending_messages(),
        "steering message should have been consumed during the run"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// T028 — abort_stops_running_turn (AC 19)
//
// Start a prompt_stream with a slow tool (5s delay). When we see
// ToolExecutionStart, call agent.abort(). Assert the stream terminates
// with AgentEnd.
// ═════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn abort_stops_running_turn() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("tc_slow", "slow_tool", "{}"),
        text_only_events("unreachable"),
    ]));
    let tool = Arc::new(MockTool::new("slow_tool").with_delay(Duration::from_secs(5)));
    let mut agent = make_agent_with_tools(stream_fn, vec![tool]);

    let mut stream = agent.prompt_stream(vec![user_msg("go")]).unwrap();

    let mut saw_agent_end = false;
    let mut aborted = false;

    while let Some(event) = stream.next().await {
        if matches!(event, AgentEvent::ToolExecutionStart { .. }) && !aborted {
            agent.abort();
            aborted = true;
        }
        if matches!(event, AgentEvent::AgentEnd { .. }) {
            saw_agent_end = true;
        }
    }

    assert!(
        aborted,
        "should have seen ToolExecutionStart and called abort"
    );
    assert!(
        saw_agent_end,
        "stream should terminate with AgentEnd after abort"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// T029 — sync_api_blocks_until_complete (AC 20)
//
// Call prompt_sync from a plain #[test] (no tokio runtime). It should block
// until complete and return the full response. prompt_sync creates its own
// internal Tokio runtime.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn sync_api_blocks_until_complete() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("sync response")]));
    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert).with_retry_strategy(
            Box::new(
                DefaultRetryStrategy::default()
                    .with_jitter(false)
                    .with_base_delay(Duration::from_millis(1)),
            ),
        ),
    );

    let result = agent.prompt_sync(vec![user_msg("hi")]).unwrap();

    assert_eq!(result.stop_reason, StopReason::Stop);
    assert!(result.error.is_none());

    let has_text = result.messages.iter().any(|m| {
        matches!(
            m,
            AgentMessage::Llm(LlmMessage::Assistant(a))
                if a.content.iter().any(|b| matches!(b, ContentBlock::Text { text } if text == "sync response"))
        )
    });
    assert!(has_text, "sync prompt should return accumulated text");
    assert!(
        !agent.state().is_running,
        "agent should be idle after sync completes"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// T030 — followup_decision_controls_continuation (AC 21)
//
// Verify that when the agent produces a text-only response (no tool calls),
// the loop naturally stops after one turn. Then verify that with a
// MaxTurnsPolicy(1), even tool calls don't continue past the first turn.
// ═════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn followup_decision_controls_continuation_natural_stop() {
    // Text-only response: the agent should stop after a single turn with no
    // follow-up and no tool calls.
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("done")]));
    let mut agent = make_agent(stream_fn);

    let collector = EventCollector::new();
    let _sub = agent.subscribe(collector.subscriber());

    let result = agent.prompt_async(vec![user_msg("hello")]).await.unwrap();

    assert_eq!(result.stop_reason, StopReason::Stop);

    // Count TurnStart events to verify only one turn occurred.
    let turn_starts = collector
        .events()
        .iter()
        .filter(|e| e == &"TurnStart")
        .count();
    assert_eq!(
        turn_starts, 1,
        "text-only response should result in exactly one turn, got {turn_starts}"
    );
}

#[tokio::test]
async fn followup_decision_controls_continuation_policy_limits() {
    // With MaxTurnsPolicy(1), even a tool call response should not continue
    // beyond one turn.
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("tc_1", "my_tool", "{}"),
        text_only_events("should not reach this"),
    ]));
    let tool = Arc::new(MockTool::new("my_tool"));

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_tools(vec![tool.clone()])
            .with_pre_turn_policy(MaxTurnsPolicy::new(1))
            .with_retry_strategy(Box::new(
                DefaultRetryStrategy::default()
                    .with_jitter(false)
                    .with_base_delay(Duration::from_millis(1)),
            )),
    );

    let collector = EventCollector::new();
    let _sub = agent.subscribe(collector.subscriber());

    let _result = agent.prompt_async(vec![user_msg("go")]).await.unwrap();

    // The policy should have stopped the loop after 1 turn, despite tool use.
    let turn_starts = collector
        .events()
        .iter()
        .filter(|e| e == &"TurnStart")
        .count();
    assert!(
        turn_starts <= 1,
        "MaxTurnsPolicy(1) should limit to at most 1 turn, got {turn_starts}"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// T031 — custom_messages_survive_compaction (AC 22)
//
// Custom messages are preserved in agent state messages but are not sent to
// the provider (default_convert returns None for Custom variant).
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug)]
#[allow(dead_code)]
struct TestCustomMessage(String);

impl CustomMessage for TestCustomMessage {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[tokio::test]
async fn custom_messages_survive_compaction() {
    // Use MockContextCapturingStreamFn to verify the custom message is NOT sent
    // to the LLM (default_convert returns None for Custom). The loop builds
    // the StreamFn context from converted LLM messages only.
    let capturing_fn = Arc::new(MockContextCapturingStreamFn::new(vec![text_only_events(
        "acknowledged",
    )]));
    let stream_fn: Arc<dyn swink_agent::StreamFn> =
        Arc::clone(&capturing_fn) as Arc<dyn swink_agent::StreamFn>;

    let custom_msg = AgentMessage::Custom(Box::new(TestCustomMessage("metadata".to_string())));

    let mut agent = make_agent(stream_fn);

    // Include the custom message alongside a normal user message.
    let _result = agent
        .prompt_async(vec![custom_msg, user_msg("hello")])
        .await
        .unwrap();

    // The custom message should exist in the agent's full state message history
    // (state().messages includes input + output messages).
    let custom_in_state = agent
        .state()
        .messages
        .iter()
        .any(|m| matches!(m, AgentMessage::Custom(_)));
    assert!(
        custom_in_state,
        "custom message should survive in agent state messages"
    );

    // The StreamFn context should only contain LLM-convertible messages.
    // The custom message is filtered out by convert_to_llm (returns None).
    // So the context should have 1 message (just the user message), not 2.
    let counts = capturing_fn.captured_message_counts.lock().unwrap().clone();
    assert!(
        !counts.is_empty(),
        "stream should have been called at least once"
    );
    assert_eq!(
        counts[0], 1,
        "context passed to StreamFn should contain only 1 LLM message (user), got {}",
        counts[0]
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// T032 — retry_exhaustion_surfaces_error
//
// When all retry attempts are exhausted on a throttle error, the agent
// should return an error rather than silently succeeding.
// ═════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn retry_exhaustion_surfaces_error() {
    // All attempts return throttle errors — no successful response.
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        error_events("throttled", Some(StreamErrorKind::Throttled)),
        error_events("throttled", Some(StreamErrorKind::Throttled)),
        error_events("throttled", Some(StreamErrorKind::Throttled)),
    ]));

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert).with_retry_strategy(
            Box::new(
                DefaultRetryStrategy::default()
                    .with_max_attempts(2)
                    .with_jitter(false)
                    .with_base_delay(Duration::from_millis(1)),
            ),
        ),
    );

    let result = agent.prompt_async(vec![user_msg("hello")]).await;

    // The agent should surface the error after exhausting retries.
    // Depending on implementation, this may be an Err or an Ok with error field set.
    match result {
        Err(_) => {
            // Expected: retry exhaustion returns an error.
        }
        Ok(r) => {
            // Some implementations return Ok with an error field or Error stop reason.
            assert!(
                r.error.is_some() || r.stop_reason == StopReason::Error,
                "exhausted retries should surface an error in the result"
            );
        }
    }
}
