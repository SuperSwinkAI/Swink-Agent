//! End-to-end integration tests verifying that context compaction actually
//! happens after overflow, not just that the overflow flag is passed.

mod common;

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{
    MockContextCapturingStreamFn, MockTool, default_exhausted_fallback, default_model,
    next_response, text_only_events, tool_call_events, user_msg,
};
use futures::Stream;
use futures::stream::StreamExt;
use tokio_util::sync::CancellationToken;

use swink_agent::{
    AgentContext, AgentEvent, AgentLoopConfig, AgentMessage, AssistantMessageEvent, ContentBlock,
    DefaultRetryStrategy, LlmMessage, ModelSpec, StopReason, StreamFn, StreamOptions, UserMessage,
    agent_loop, sliding_window,
};

// ─── MockMessageCapturingStreamFn ────────────────────────────────────────────

/// A `StreamFn` that captures the full LLM messages on each call.
struct MockMessageCapturingStreamFn {
    responses: Mutex<Vec<Vec<AssistantMessageEvent>>>,
    captured_messages: Arc<Mutex<Vec<Vec<LlmMessage>>>>,
}

impl StreamFn for MockMessageCapturingStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a ModelSpec,
        context: &'a AgentContext,
        _options: &'a StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        let llm_msgs: Vec<LlmMessage> = context
            .messages
            .iter()
            .filter_map(|m| match m {
                AgentMessage::Llm(llm) => Some(llm.clone()),
                AgentMessage::Custom(_) => None,
            })
            .collect();
        self.captured_messages.lock().unwrap().push(llm_msgs);

        let events = next_response(&self.responses, default_exhausted_fallback());
        Box::pin(futures::stream::iter(events))
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────

fn overflow_error_events() -> Vec<AssistantMessageEvent> {
    vec![AssistantMessageEvent::Error {
        stop_reason: StopReason::Error,
        error_message: "context_length_exceeded: too many tokens".to_string(),
        usage: None,
        error_kind: None,
    }]
}

type ConvertToLlmBoxed = Box<dyn Fn(&AgentMessage) -> Option<LlmMessage> + Send + Sync>;

fn default_convert_to_llm() -> ConvertToLlmBoxed {
    Box::new(|msg: &AgentMessage| match msg {
        AgentMessage::Llm(llm) => Some(llm.clone()),
        AgentMessage::Custom(_) => None,
    })
}

fn default_config(stream_fn: Arc<dyn StreamFn>) -> AgentLoopConfig {
    AgentLoopConfig {
        model: default_model(),
        stream_options: StreamOptions::default(),
        retry_strategy: Box::new(
            DefaultRetryStrategy::default()
                .with_jitter(false)
                .with_base_delay(Duration::from_millis(1)),
        ),
        stream_fn,
        tools: vec![],
        convert_to_llm: default_convert_to_llm(),
        transform_context: None,
        get_api_key: None,
        message_provider: None,
        approve_tool: None,
        approval_mode: swink_agent::ApprovalMode::default(),
        pre_turn_policies: vec![],
        pre_dispatch_policies: vec![],
        post_turn_policies: vec![],
        post_loop_policies: vec![],
        async_transform_context: None,
        metrics_collector: None,
        fallback: None,
        tool_execution_policy: swink_agent::ToolExecutionPolicy::default(),
    }
}

/// Create a large user message (~`token_count` estimated tokens).
/// Token estimation is chars / 4, so we create 4 * `token_count` chars.
fn large_user_msg(label: &str, token_count: usize) -> AgentMessage {
    let padding = "x".repeat(token_count * 4);
    let text = format!("{label}:{padding}");
    AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text { text }],
        timestamp: 0,
    }))
}

/// Collect all events from a loop stream.
async fn collect_events(stream: Pin<Box<dyn Stream<Item = AgentEvent> + Send>>) -> Vec<AgentEvent> {
    stream.collect().await
}

/// Check if events contain a specific variant (by Debug name prefix).
fn has_event(events: &[AgentEvent], name: &str) -> bool {
    events.iter().any(|e| format!("{e:?}").starts_with(name))
}

// ═══════════════════════════════════════════════════════════════════════════
// Test a: overflow_triggers_compaction
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn overflow_triggers_compaction() {
    // Set up a MockContextCapturingStreamFn:
    //   Call 1: returns overflow error (context_length_exceeded)
    //   Call 2: succeeds with text
    let capturing_fn = Arc::new(MockContextCapturingStreamFn::new(vec![
        overflow_error_events(),
        text_only_events("recovered"),
    ]));
    let stream_fn: Arc<dyn StreamFn> = Arc::clone(&capturing_fn) as Arc<dyn StreamFn>;

    // Track overflow flags passed to transform_context
    let overflow_flags: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));
    let flags_clone = Arc::clone(&overflow_flags);

    let mut config = default_config(stream_fn);
    // Use sliding_window with a budget that forces compaction on overflow.
    // Normal budget: 10000 (generous), overflow budget: 200 (tight).
    let compact = sliding_window(10_000, 200, 1);
    config.transform_context = Some(Arc::new(
        move |msgs: &mut Vec<AgentMessage>, overflow: bool| {
            flags_clone.lock().unwrap().push(overflow);
            compact(msgs, overflow);
        },
    ));

    // Provide many large messages so that overflow compaction has work to do.
    // Each message is ~100 tokens (400 chars / 4).
    let mut initial_messages = Vec::new();
    for i in 0..10 {
        initial_messages.push(large_user_msg(&format!("msg{i}"), 100));
    }

    let events = collect_events(agent_loop(
        initial_messages,
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"), "loop should complete");

    // Verify overflow flag was set on the second call
    let flags: Vec<bool> = overflow_flags.lock().unwrap().clone();
    assert!(flags.len() >= 2, "transform_context called at least twice");
    assert!(!flags[0], "first call should not have overflow");
    assert!(flags[1], "second call should have overflow=true");

    // Verify context was actually smaller on the second stream call
    let counts: Vec<usize> = capturing_fn.captured_message_counts.lock().unwrap().clone();
    assert!(
        counts.len() >= 2,
        "stream should be called at least twice, got {}",
        counts.len()
    );
    assert!(
        counts[1] < counts[0],
        "context should be smaller after compaction: first={}, second={}",
        counts[0],
        counts[1]
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Test b: compacted_context_preserves_anchors
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn compacted_context_preserves_anchors() {
    let captured_messages: Arc<Mutex<Vec<Vec<LlmMessage>>>> = Arc::new(Mutex::new(Vec::new()));

    let stream_fn = Arc::new(MockMessageCapturingStreamFn {
        responses: Mutex::new(vec![overflow_error_events(), text_only_events("ok")]),
        captured_messages: Arc::clone(&captured_messages),
    });

    let anchor_count = 2;
    let compact = sliding_window(10_000, 300, anchor_count);

    let mut config = default_config(stream_fn as Arc<dyn StreamFn>);
    config.transform_context = Some(Arc::new(
        move |msgs: &mut Vec<AgentMessage>, overflow: bool| {
            compact(msgs, overflow);
        },
    ));

    // Create messages: first two are anchors, rest are filler.
    let mut initial_messages = vec![user_msg("ANCHOR_ONE"), user_msg("ANCHOR_TWO")];
    for i in 0..8 {
        initial_messages.push(large_user_msg(&format!("filler{i}"), 100));
    }

    let events = collect_events(agent_loop(
        initial_messages,
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"));

    let all_captured: Vec<Vec<LlmMessage>> = captured_messages.lock().unwrap().clone();
    assert!(
        all_captured.len() >= 2,
        "should have at least 2 stream calls"
    );

    // After overflow compaction (second call), the first two messages should
    // still be the anchors.
    let post_overflow = &all_captured[1];
    assert!(
        post_overflow.len() >= anchor_count,
        "post-overflow context should have at least {anchor_count} messages, got {}",
        post_overflow.len()
    );

    // Verify anchor messages survived
    let first_text = match &post_overflow[0] {
        LlmMessage::User(u) => ContentBlock::extract_text(&u.content),
        LlmMessage::Assistant(_) | LlmMessage::ToolResult(_) => String::new(),
    };
    let second_text = match &post_overflow[1] {
        LlmMessage::User(u) => ContentBlock::extract_text(&u.content),
        LlmMessage::Assistant(_) | LlmMessage::ToolResult(_) => String::new(),
    };
    assert!(
        first_text.contains("ANCHOR_ONE"),
        "first anchor should survive compaction, got: {first_text}"
    );
    assert!(
        second_text.contains("ANCHOR_TWO"),
        "second anchor should survive compaction, got: {second_text}"
    );

    // Context should be smaller than original
    assert!(
        post_overflow.len() < 10,
        "context should be compacted, got {} messages",
        post_overflow.len()
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Test c: compacted_context_preserves_tool_pairs
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn compacted_context_preserves_tool_pairs() {
    let captured_messages: Arc<Mutex<Vec<Vec<LlmMessage>>>> = Arc::new(Mutex::new(Vec::new()));

    // First call: returns a tool call. Second call: overflow. Third call: success.
    let stream_fn = Arc::new(MockMessageCapturingStreamFn {
        responses: Mutex::new(vec![
            tool_call_events("tc_1", "mock_tool", "{}"),
            overflow_error_events(),
            text_only_events("done"),
        ]),
        captured_messages: Arc::clone(&captured_messages),
    });

    let tool = Arc::new(MockTool::new("mock_tool"));
    // Budget: overflow_budget tight enough to trigger compaction but large enough
    // to keep the tool pair (tool call + result are relatively small).
    let compact = sliding_window(10_000, 500, 1);

    let mut config = default_config(stream_fn as Arc<dyn StreamFn>);
    config.tools = vec![tool];
    config.transform_context = Some(Arc::new(
        move |msgs: &mut Vec<AgentMessage>, overflow: bool| {
            compact(msgs, overflow);
        },
    ));

    // Start with filler + the prompt. The loop will add an assistant (tool call)
    // and a tool result, then on the second turn, trigger overflow.
    let mut initial_messages = Vec::new();
    for i in 0..6 {
        initial_messages.push(large_user_msg(&format!("filler{i}"), 100));
    }

    let events = collect_events(agent_loop(
        initial_messages,
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"));

    let post_overflow = {
        let captured = captured_messages.lock().unwrap();
        // After the overflow (third stream call), check that tool pairs survive.
        assert!(
            captured.len() >= 3,
            "should have at least 3 stream calls, got {}",
            captured.len()
        );
        captured[2].clone()
    };

    // Check that if a ToolResult is present, its corresponding Assistant with
    // ToolCall is also present.
    let has_tool_result = post_overflow
        .iter()
        .any(|m| matches!(m, LlmMessage::ToolResult(_)));
    let has_tool_call = post_overflow.iter().any(|m| {
        matches!(m, LlmMessage::Assistant(a)
            if a.content.iter().any(|b| matches!(b, ContentBlock::ToolCall { .. })))
    });

    if has_tool_result {
        assert!(
            has_tool_call,
            "tool result survived compaction but its tool call did not"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Test d: multiple_overflows_progressively_shrink
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn multiple_overflows_progressively_shrink() {
    let capturing_fn = Arc::new(MockContextCapturingStreamFn::new(vec![
        overflow_error_events(),
        overflow_error_events(),
        text_only_events("recovered"),
    ]));
    let stream_fn: Arc<dyn StreamFn> = Arc::clone(&capturing_fn) as Arc<dyn StreamFn>;

    // Use a transform that progressively removes more messages each time
    // overflow is signaled. We simulate this by using a counter that
    // tightens the budget on each overflow.
    let overflow_count: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let overflow_clone = Arc::clone(&overflow_count);

    let mut config = default_config(stream_fn);
    config.transform_context = Some(Arc::new(
        move |msgs: &mut Vec<AgentMessage>, overflow: bool| {
            if overflow {
                let n = {
                    let mut count = overflow_clone.lock().unwrap();
                    *count += 1;
                    *count
                };
                // Each overflow removes progressively more: keep only the last
                // (msgs.len() - n * 2) messages, minimum 1.
                let keep = msgs.len().saturating_sub(n * 2).max(1);
                if keep < msgs.len() {
                    let tail: Vec<AgentMessage> = msgs.drain(keep..).collect();
                    msgs.clear();
                    msgs.extend(tail);
                }
            }
        },
    ));

    // Start with many messages
    let mut initial_messages = Vec::new();
    for i in 0..10 {
        initial_messages.push(large_user_msg(&format!("msg{i}"), 50));
    }

    let events = collect_events(agent_loop(
        initial_messages,
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"));

    let counts: Vec<usize> = capturing_fn.captured_message_counts.lock().unwrap().clone();
    assert!(
        counts.len() >= 3,
        "should have at least 3 stream calls, got {}",
        counts.len()
    );

    // Each successive call should see fewer messages
    assert!(
        counts[1] < counts[0],
        "second call should have fewer messages than first: {} vs {}",
        counts[1],
        counts[0]
    );
    assert!(
        counts[2] < counts[1],
        "third call should have fewer messages than second: {} vs {}",
        counts[2],
        counts[1]
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Test e: overflow_with_single_large_message
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn overflow_with_single_large_message() {
    // When a single message exceeds the budget, sliding_window cannot compact
    // further. The loop must not enter an infinite retry cycle. The second
    // stream call should succeed (simulating the provider accepting after prune).
    let capturing_fn = Arc::new(MockContextCapturingStreamFn::new(vec![
        overflow_error_events(),
        text_only_events("handled gracefully"),
    ]));
    let stream_fn: Arc<dyn StreamFn> = Arc::clone(&capturing_fn) as Arc<dyn StreamFn>;

    // Very tight overflow budget that a single large message exceeds.
    let compact = sliding_window(10_000, 10, 1);

    let mut config = default_config(stream_fn);
    config.transform_context = Some(Arc::new(
        move |msgs: &mut Vec<AgentMessage>, overflow: bool| {
            compact(msgs, overflow);
        },
    ));

    // One large message (~500 tokens, well above the 10-token overflow budget).
    let initial_messages = vec![large_user_msg("huge", 500)];

    let events = collect_events(agent_loop(
        initial_messages,
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    // The loop should terminate (not hang) and produce AgentEnd.
    assert!(
        has_event(&events, "AgentEnd"),
        "loop should complete even when a single message exceeds the budget"
    );

    assert!(
        capturing_fn.captured_message_counts.lock().unwrap().len() >= 2,
        "should have at least 2 stream calls (overflow + recovery)"
    );
}
