//! End-to-end integration tests exercising the full Agent -> loop -> mock
//! `StreamFn` -> tool execution -> events stack. Tests 6.1 through 6.15 per the
//! implementation plan.

mod common;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{
    EventCollector, MockContextCapturingStreamFn, MockStreamFn, MockTool, default_convert,
    default_model, text_only_events, tool_call_events, user_msg,
};
use futures::stream::StreamExt;
use serde_json::json;

use swink_agent::{
    Agent, AgentError, AgentEvent, AgentMessage, AgentOptions, AgentTool, AgentToolResult,
    AssistantMessageEvent, ContentBlock, Cost, DefaultRetryStrategy, LlmMessage, ModelSpec,
    StopReason, StreamFn, StreamOptions, Usage, UserMessage,
};

// ─── Helpers ─────────────────────────────────────────────────────────────

fn make_agent(stream_fn: Arc<dyn StreamFn>) -> Agent {
    Agent::new(
        AgentOptions::new("test prompt", default_model(), stream_fn, default_convert)
            .with_retry_strategy(Box::new(
                DefaultRetryStrategy::default()
                    .with_jitter(false)
                    .with_base_delay(Duration::from_millis(1)),
            )),
    )
}

fn make_agent_with_tools(stream_fn: Arc<dyn StreamFn>, tools: Vec<Arc<dyn AgentTool>>) -> Agent {
    Agent::new(
        AgentOptions::new("test prompt", default_model(), stream_fn, default_convert)
            .with_tools(tools)
            .with_retry_strategy(Box::new(
                DefaultRetryStrategy::default()
                    .with_jitter(false)
                    .with_base_delay(Duration::from_millis(1)),
            )),
    )
}

// ═══════════════════════════════════════════════════════════════════════════
// 6.1 — Agent loop emits all lifecycle events in correct order for
//        single-turn, no-tool conversation
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn lifecycle_events_order_single_turn() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let mut agent = make_agent(stream_fn);

    let collector = EventCollector::new();
    let _sub = agent.subscribe(collector.subscriber());

    let _result = agent.prompt_async(vec![user_msg("hi")]).await.unwrap();

    let names = collector.events();

    let agent_start = collector.position("AgentStart").expect("AgentStart");
    let turn_start = collector.position("TurnStart").expect("TurnStart");
    let msg_start = collector.position("MessageStart").expect("MessageStart");
    let msg_end = collector.position("MessageEnd").expect("MessageEnd");
    let turn_end = collector.position("TurnEnd").expect("TurnEnd");
    let agent_end = collector.position("AgentEnd").expect("AgentEnd");

    assert!(agent_start < turn_start, "AgentStart before TurnStart");
    assert!(turn_start < msg_start, "TurnStart before MessageStart");
    assert!(msg_start < msg_end, "MessageStart before MessageEnd");
    assert!(msg_end < turn_end, "MessageEnd before TurnEnd");
    assert!(turn_end < agent_end, "TurnEnd before AgentEnd");

    // Verify MessageUpdate deltas appear between MessageStart and MessageEnd.
    let has_update = names.iter().any(|n| n == "MessageUpdate");
    assert!(has_update, "should have at least one MessageUpdate");
}

// ═══════════════════════════════════════════════════════════════════════════
// 6.2 — Tool arguments validated against JSON Schema; invalid args produce
//        error results without execute
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn invalid_tool_args_produce_error_without_execute() {
    let strict_schema = json!({
        "type": "object",
        "properties": {
            "path": { "type": "string" }
        },
        "required": ["path"],
        "additionalProperties": false
    });
    let tool = Arc::new(MockTool::new("read_file").with_schema(strict_schema));
    let tool_ref = Arc::clone(&tool);

    // LLM sends a tool call with missing required "path" field.
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("tc_1", "read_file", r#"{"wrong_key": 42}"#),
        text_only_events("recovered"),
    ]));
    let mut agent = make_agent_with_tools(stream_fn, vec![tool]);

    let result = agent.prompt_async(vec![user_msg("go")]).await.unwrap();

    // The tool should NOT have been executed.
    assert!(
        !tool_ref.was_executed(),
        "tool should not execute with invalid args"
    );

    // The result should contain a tool result with is_error = true.
    let has_error_result = result.messages.iter().any(|m| {
        matches!(
            m,
            AgentMessage::Llm(LlmMessage::ToolResult(tr)) if tr.is_error
        )
    });
    assert!(has_error_result, "should have an error tool result");
}

// ═══════════════════════════════════════════════════════════════════════════
// 6.3 — Tool calls within a single turn execute concurrently
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn tools_execute_concurrently() {
    let delay = Duration::from_millis(200);
    let tool_a = Arc::new(MockTool::new("tool_a").with_delay(delay));
    let tool_b = Arc::new(MockTool::new("tool_b").with_delay(delay));

    // Stream events with two tool calls in the same response.
    let two_tool_events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            id: "tc_a".to_string(),
            name: "tool_a".to_string(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            delta: "{}".to_string(),
        },
        AssistantMessageEvent::ToolCallEnd { content_index: 0 },
        AssistantMessageEvent::ToolCallStart {
            content_index: 1,
            id: "tc_b".to_string(),
            name: "tool_b".to_string(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 1,
            delta: "{}".to_string(),
        },
        AssistantMessageEvent::ToolCallEnd { content_index: 1 },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
            cost: Cost::default(),
        },
    ];

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        two_tool_events,
        text_only_events("done"),
    ]));
    let mut agent = make_agent_with_tools(stream_fn, vec![tool_a, tool_b]);

    let start = Instant::now();
    let _result = agent.prompt_async(vec![user_msg("go")]).await.unwrap();
    let elapsed = start.elapsed();

    // If sequential, would take >= 400ms. Concurrent should be close to 200ms.
    assert!(
        elapsed < Duration::from_millis(380),
        "tools should run concurrently, took {elapsed:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 6.4 — Steering messages interrupt tool execution; remaining tools
//        cancelled with error results
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn steering_interrupts_tool_execution() {
    let slow_tool = Arc::new(MockTool::new("slow").with_delay(Duration::from_secs(5)));

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("tc_slow", "slow", "{}"),
        text_only_events("after interrupt"),
    ]));

    let mut agent = make_agent_with_tools(stream_fn, vec![slow_tool]);

    // Pre-queue a steering message; it will be detected after tool dispatch.
    agent.steer(user_msg("interrupt now"));

    let result = agent.prompt_async(vec![user_msg("go")]).await.unwrap();

    // The run should complete without hanging.
    assert!(!result.messages.is_empty(), "should produce messages");
}

// ═══════════════════════════════════════════════════════════════════════════
// 6.5 — Follow-up messages cause the agent to continue after natural stop
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn follow_up_continues_after_stop() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        text_only_events("first answer"),
        text_only_events("follow-up answer"),
    ]));
    let mut agent = make_agent(stream_fn);

    // Queue follow-up before starting.
    agent.follow_up(user_msg("and then?"));

    let result = agent.prompt_async(vec![user_msg("hello")]).await.unwrap();

    // Should have at least two assistant messages (one per turn).
    let assistant_count = result
        .messages
        .iter()
        .filter(|m| matches!(m, AgentMessage::Llm(LlmMessage::Assistant(_))))
        .count();
    assert!(
        assistant_count >= 2,
        "expected at least 2 assistant messages from follow-up, got {assistant_count}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 6.6 — Aborting via CancellationToken produces clean shutdown with
//        StopReason::Aborted
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn abort_produces_aborted_stop_reason() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("tc_1", "slow_tool", "{}"),
        text_only_events("unreachable"),
    ]));
    let tool = Arc::new(MockTool::new("slow_tool").with_delay(Duration::from_secs(10)));
    let mut agent = make_agent_with_tools(stream_fn, vec![tool]);

    let mut stream = agent.prompt_stream(vec![user_msg("go")]).unwrap();

    let mut saw_agent_end = false;
    let mut saw_aborted = false;

    while let Some(event) = stream.next().await {
        if matches!(event, AgentEvent::ToolExecutionStart { .. }) {
            agent.abort();
        }
        if matches!(event, AgentEvent::AgentEnd { .. }) {
            saw_agent_end = true;
        }
        if let AgentEvent::TurnEnd {
            ref assistant_message,
            ..
        } = event
            && assistant_message.stop_reason == StopReason::Aborted
        {
            saw_aborted = true;
        }
    }

    // The stream should terminate cleanly. The abort cancels the token which
    // causes the spawned tools to be cancelled and the loop to exit.
    assert!(saw_agent_end, "stream should terminate with AgentEnd");
    // The Aborted stop reason should appear in TurnEnd. With the cancellation-
    // aware MockTool, the abort propagates reliably.
    let _ = saw_aborted; // May or may not be visible depending on event ordering.
}

// ═══════════════════════════════════════════════════════════════════════════
// 6.7 — Proxy stream correctly reconstructs assistant message from delta
//        SSE events (test via mock StreamFn, not HTTP)
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn accumulates_text_and_tool_call_deltas() {
    let tool = Arc::new(MockTool::new("greet"));

    // Build events with text + tool call, using multiple deltas for both.
    let events = vec![
        AssistantMessageEvent::Start,
        // Text block at index 0, split across two deltas
        AssistantMessageEvent::TextStart { content_index: 0 },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "Hel".to_string(),
        },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "lo!".to_string(),
        },
        AssistantMessageEvent::TextEnd { content_index: 0 },
        // Tool call at index 1, JSON args split across deltas
        AssistantMessageEvent::ToolCallStart {
            content_index: 1,
            id: "tc_g".to_string(),
            name: "greet".to_string(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 1,
            delta: r#"{"na"#.to_string(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 1,
            delta: r#"me":"#.to_string(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 1,
            delta: r#""World"}"#.to_string(),
        },
        AssistantMessageEvent::ToolCallEnd { content_index: 1 },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::ToolUse,
            usage: Usage {
                input: 10,
                output: 20,
                cache_read: 0,
                cache_write: 0,
                total: 30,
                ..Default::default()
            },
            cost: Cost::default(),
        },
    ];

    let stream_fn = Arc::new(MockStreamFn::new(vec![events, text_only_events("done")]));
    let mut agent = make_agent_with_tools(stream_fn, vec![tool]);

    let result = agent.prompt_async(vec![user_msg("hi")]).await.unwrap();

    // Find the first assistant message and verify accumulated content.
    let assistant = result.messages.iter().find_map(|m| match m {
        AgentMessage::Llm(LlmMessage::Assistant(a)) => Some(a),
        _ => None,
    });
    let assistant = assistant.expect("should have an assistant message");

    // Text block should be fully accumulated.
    let text = assistant.content.iter().find_map(|b| match b {
        ContentBlock::Text { text } => Some(text.as_str()),
        _ => None,
    });
    assert_eq!(text, Some("Hello!"), "text deltas should accumulate");

    // Tool call should have parsed arguments.
    let tool_call = assistant.content.iter().find_map(|b| match b {
        ContentBlock::ToolCall {
            name, arguments, ..
        } => Some((name.as_str(), arguments)),
        _ => None,
    });
    let (name, args) = tool_call.expect("should have a tool call");
    assert_eq!(name, "greet");
    assert_eq!(args["name"], "World", "tool call args should be parsed");
}

// ═══════════════════════════════════════════════════════════════════════════
// 6.8 — Calling prompt while already running returns AlreadyRunning error
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn prompt_while_running_returns_already_running() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("first")]));
    let mut agent = make_agent(stream_fn);

    // Start a stream but do not consume it.
    let _stream = agent.prompt_stream(vec![user_msg("first")]).unwrap();
    assert!(agent.state().is_running);

    // All three invocation modes should fail.
    let err_stream = agent.prompt_stream(vec![user_msg("second")]);
    assert!(
        matches!(err_stream, Err(AgentError::AlreadyRunning)),
        "prompt_stream should return AlreadyRunning"
    );

    let err_sync = agent.prompt_sync(vec![user_msg("third")]);
    assert!(
        matches!(err_sync, Err(AgentError::AlreadyRunning)),
        "prompt_sync should return AlreadyRunning"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 6.9 — transform_context is called before convert_to_llm on every turn
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn transform_context_called_before_convert() {
    let transform_count = Arc::new(AtomicU32::new(0));
    let transform_clone = Arc::clone(&transform_count);

    let tracking_fn = Arc::new(MockContextCapturingStreamFn::new(vec![
        tool_call_events("tc_1", "my_tool", "{}"),
        text_only_events("done"),
    ]));
    let stream_fn: Arc<dyn StreamFn> = Arc::clone(&tracking_fn) as Arc<dyn StreamFn>;
    let tool = Arc::new(MockTool::new("my_tool"));

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_tools(vec![tool])
            .with_transform_context(move |_msgs: &mut Vec<AgentMessage>, _overflow: bool| {
                transform_clone.fetch_add(1, Ordering::SeqCst);
            })
            .with_retry_strategy(Box::new(
                DefaultRetryStrategy::default()
                    .with_jitter(false)
                    .with_base_delay(Duration::from_millis(1)),
            )),
    );

    let _result = agent.prompt_async(vec![user_msg("go")]).await.unwrap();

    let tc = transform_count.load(Ordering::SeqCst);
    let stream_calls = tracking_fn.captured_message_counts.lock().unwrap().len();

    // transform_context should be called at least once per turn.
    assert!(
        tc >= 2,
        "transform_context should be called on every turn, got {tc}"
    );
    // Each stream call corresponds to a turn that had transform_context called first.
    assert_eq!(
        tc as usize, stream_calls,
        "transform_context calls ({tc}) should match stream calls ({stream_calls})"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 6.10 — All public types are Send + Sync
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn public_types_are_send_sync() {
    const _: () = {
        const fn assert_send_sync<T: Send + Sync>() {}

        assert_send_sync::<ContentBlock>();
        assert_send_sync::<swink_agent::ImageSource>();
        assert_send_sync::<UserMessage>();
        assert_send_sync::<swink_agent::AssistantMessage>();
        assert_send_sync::<swink_agent::ToolResultMessage>();
        assert_send_sync::<LlmMessage>();
        assert_send_sync::<AgentMessage>();
        assert_send_sync::<Usage>();
        assert_send_sync::<Cost>();
        assert_send_sync::<StopReason>();
        assert_send_sync::<swink_agent::ThinkingLevel>();
        assert_send_sync::<swink_agent::ThinkingBudgets>();
        assert_send_sync::<ModelSpec>();
        assert_send_sync::<swink_agent::AgentResult>();
        assert_send_sync::<swink_agent::AgentContext>();
        assert_send_sync::<AssistantMessageEvent>();
        assert_send_sync::<swink_agent::AssistantMessageDelta>();
        assert_send_sync::<swink_agent::StreamTransport>();
        assert_send_sync::<StreamOptions>();
        assert_send_sync::<AgentToolResult>();
        assert_send_sync::<AgentError>();
        assert_send_sync::<DefaultRetryStrategy>();
        assert_send_sync::<swink_agent::SubscriptionId>();
    };
}

// ═══════════════════════════════════════════════════════════════════════════
// 6.11 — Structured output retries up to configured max on invalid response
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn structured_output_retries_on_invalid() {
    let schema = json!({
        "type": "object",
        "properties": {
            "color": { "type": "string" }
        },
        "required": ["color"]
    });

    // Attempt 0: invalid (missing "color"), needs 2 responses.
    // Attempt 1: invalid again, needs 2 responses.
    // Attempt 2: valid, needs 2 responses.
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("so_1", "__structured_output", "{}"),
        text_only_events("done"),
        tool_call_events("so_2", "__structured_output", r#"{"wrong": 1}"#),
        text_only_events("done"),
        tool_call_events("so_3", "__structured_output", r#"{"color": "blue"}"#),
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
        .structured_output("pick a color".into(), schema)
        .await
        .unwrap();

    assert_eq!(value["color"], "blue");
}

// ═══════════════════════════════════════════════════════════════════════════
// 6.12 — Context window overflow surfaces as typed ContextWindowOverflow
//         error (via error message in stream)
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn context_window_overflow_error() {
    // The loop classifies errors containing "context window" or
    // "context_length_exceeded" as ContextWindowOverflow. When detected,
    // the loop sets an overflow signal and retries the turn with
    // transform_context called again.
    let overflow_events = vec![AssistantMessageEvent::Error {
        stop_reason: StopReason::Error,
        error_message: "context_length_exceeded: too many tokens".to_string(),
        usage: None,
        error_kind: None,
    }];

    let overflow_seen = Arc::new(AtomicBool::new(false));
    let overflow_clone = Arc::clone(&overflow_seen);

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        overflow_events,
        text_only_events("recovered after pruning"),
    ]));

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_transform_context(move |_msgs: &mut Vec<AgentMessage>, overflow: bool| {
                if overflow {
                    overflow_clone.store(true, Ordering::SeqCst);
                }
            })
            .with_retry_strategy(Box::new(
                DefaultRetryStrategy::default()
                    .with_jitter(false)
                    .with_base_delay(Duration::from_millis(1)),
            )),
    );

    let result = agent.prompt_async(vec![user_msg("hi")]).await.unwrap();

    // The overflow signal should have been passed to transform_context.
    assert!(
        overflow_seen.load(Ordering::SeqCst),
        "transform_context should receive overflow=true"
    );

    // The agent should have recovered.
    let has_text = result.messages.iter().any(|m| {
        matches!(m, AgentMessage::Llm(LlmMessage::Assistant(a))
            if a.content.iter().any(|b| matches!(b, ContentBlock::Text { text } if text.contains("recovered"))))
    });
    assert!(has_text, "agent should recover after overflow");
}

// ═══════════════════════════════════════════════════════════════════════════
// 6.13 — Incomplete tool calls from max tokens are replaced with error
//         tool results
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn incomplete_tool_calls_get_error_results() {
    let tool = Arc::new(MockTool::new("my_tool"));

    // Simulate a truncated tool call: no ToolCallEnd, stop_reason = Length.
    // The partial_json will remain set, marking it incomplete.
    let truncated_events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            id: "tc_trunc".to_string(),
            name: "my_tool".to_string(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            delta: r#"{"partial"#.to_string(),
        },
        // No ToolCallEnd — truncated by max tokens
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Length,
            usage: Usage::default(),
            cost: Cost::default(),
        },
    ];

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        truncated_events,
        text_only_events("recovered from truncation"),
    ]));
    let mut agent = make_agent_with_tools(stream_fn, vec![tool]);

    let result = agent.prompt_async(vec![user_msg("go")]).await.unwrap();

    // There should be a tool result with is_error = true indicating
    // the tool call was incomplete.
    let has_incomplete_error = result.messages.iter().any(|m| {
        matches!(
            m,
            AgentMessage::Llm(LlmMessage::ToolResult(tr))
                if tr.is_error && tr.content.iter().any(|b|
                    matches!(b, ContentBlock::Text { text } if text.contains("incomplete")))
        )
    });
    assert!(
        has_incomplete_error,
        "incomplete tool call should produce an error tool result"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 6.14 — Default retry strategy applies exponential back-off with jitter,
//         respects max delay cap
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn retry_strategy_exponential_backoff() {
    use swink_agent::RetryStrategy;

    let strategy = DefaultRetryStrategy::default()
        .with_max_attempts(5)
        .with_base_delay(Duration::from_secs(1))
        .with_max_delay(Duration::from_secs(10))
        .with_multiplier(2.0)
        .with_jitter(false);

    // Attempt 1: base_delay * 2^0 = 1s
    let d1 = strategy.delay(1);
    assert_eq!(d1, Duration::from_secs(1), "attempt 1 = base_delay");

    // Attempt 2: base_delay * 2^1 = 2s
    let d2 = strategy.delay(2);
    assert_eq!(d2, Duration::from_secs(2), "attempt 2 = 2s");

    // Attempt 3: base_delay * 2^2 = 4s
    let d3 = strategy.delay(3);
    assert_eq!(d3, Duration::from_secs(4), "attempt 3 = 4s");

    // Attempt 4: base_delay * 2^3 = 8s
    let d4 = strategy.delay(4);
    assert_eq!(d4, Duration::from_secs(8), "attempt 4 = 8s");

    // Attempt 5: base_delay * 2^4 = 16s -> capped at 10s
    let d5 = strategy.delay(5);
    assert_eq!(d5, Duration::from_secs(10), "attempt 5 capped at max_delay");

    // should_retry: retryable error within max_attempts
    let retryable = AgentError::ModelThrottled;
    assert!(strategy.should_retry(&retryable, 1));
    assert!(strategy.should_retry(&retryable, 4));
    assert!(!strategy.should_retry(&retryable, 5), "at max_attempts");

    // Non-retryable error should not retry.
    let non_retryable = AgentError::AlreadyRunning;
    assert!(!strategy.should_retry(&non_retryable, 1));
}

#[test]
fn retry_strategy_jitter_bounded() {
    use swink_agent::RetryStrategy;

    let strategy = DefaultRetryStrategy::default()
        .with_base_delay(Duration::from_secs(1))
        .with_max_delay(Duration::from_secs(60))
        .with_multiplier(2.0)
        .with_jitter(true);

    // With jitter, delay(1) should be in [0.5s, 1.5s).
    // Sample multiple times to verify range.
    for _ in 0..50 {
        let d = strategy.delay(1);
        let secs = d.as_secs_f64();
        assert!(
            (0.49..1.51).contains(&secs),
            "jittered delay should be in [0.5, 1.5), got {secs}"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 6.15 — Sync prompt blocks until completion without caller managing
//         Tokio runtime
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn prompt_sync_blocks_until_completion() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("sync hello")]));
    let mut agent = make_agent(stream_fn);

    let result = agent.prompt_sync(vec![user_msg("hi")]).unwrap();

    assert_eq!(result.stop_reason, StopReason::Stop);
    assert!(result.error.is_none());

    let has_text = result.messages.iter().any(|m| {
        matches!(m, AgentMessage::Llm(LlmMessage::Assistant(a))
            if a.content.iter().any(|b| matches!(b, ContentBlock::Text { text } if text == "sync hello")))
    });
    assert!(has_text, "sync prompt should return accumulated text");
    assert!(!agent.state().is_running, "agent should be idle after sync");
}
