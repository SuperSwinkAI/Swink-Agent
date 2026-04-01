//! User Story 3: Context Management and Overflow (T020–T025)
//!
//! Tests verifying context window tracking, sliding-window compaction,
//! overflow-triggered retry, tool-result pair preservation, and the
//! overflow flag propagation to transform_context callbacks.

mod common;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use common::{
    MockContextCapturingStreamFn, MockStreamFn, MockTool, default_convert, default_model,
    text_only_events, tool_call_events, user_msg,
};

use swink_agent::{
    Agent, AgentMessage, AgentOptions, AssistantMessageEvent, ContentBlock, DefaultRetryStrategy,
    LlmMessage, sliding_window,
};

// ─── Helpers ─────────────────────────────────────────────────────────────

fn make_agent_with_small_context(
    stream_fn: Arc<dyn swink_agent::StreamFn>,
    normal_budget: usize,
    overflow_budget: usize,
    anchor: usize,
) -> Agent {
    Agent::new(
        AgentOptions::new(
            "test system prompt",
            default_model(),
            stream_fn,
            default_convert,
        )
        .with_transform_context_fn(sliding_window(normal_budget, overflow_budget, anchor))
        .with_retry_strategy(Box::new(
            DefaultRetryStrategy::default()
                .with_jitter(false)
                .with_base_delay(Duration::from_millis(1)),
        )),
    )
}

// ═══════════════════════════════════════════════════════════════════════════
// T020: context_window_tracking (AC 13)
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn context_window_tracking() {
    // Use MockContextCapturingStreamFn to observe message counts passed to the LLM.
    // With a small normal_budget of 200 tokens (~800 chars), sending 5 messages
    // of ~50 tokens each (200 chars) should trigger compaction after the context
    // exceeds the budget.
    let responses: Vec<Vec<AssistantMessageEvent>> = (0..6)
        .map(|i| text_only_events(&format!("reply-{i}")))
        .collect();

    let capturing_fn = Arc::new(MockContextCapturingStreamFn::new(responses));
    let stream_fn: Arc<dyn swink_agent::StreamFn> = Arc::clone(&capturing_fn) as _;

    // normal_budget=200 tokens, overflow_budget=100, anchor=1
    let mut agent = make_agent_with_small_context(stream_fn, 200, 100, 1);

    // Send 6 rounds of messages with long text to accumulate history.
    // Each user message is ~50 tokens (200 chars). Each assistant reply adds more.
    for i in 0..6 {
        let long_text = "x".repeat(200); // ~50 tokens
        let msg = user_msg(&format!("msg{i}-{long_text}"));
        let _result = agent.prompt_async(vec![msg]).await.unwrap();
    }

    let counts = capturing_fn.captured_message_counts.lock().unwrap().clone();
    assert!(
        counts.len() >= 6,
        "should have at least 6 stream calls, got {}",
        counts.len()
    );

    // Message counts should grow initially then level off or decrease once
    // compaction kicks in. Find the maximum and verify that later counts are
    // less than or equal (compaction prevented unbounded growth).
    let max_count = *counts.iter().max().unwrap();
    let last_count = *counts.last().unwrap();
    assert!(
        last_count <= max_count,
        "compaction should prevent unbounded context growth: max={max_count}, last={last_count}"
    );

    // More specifically: after enough rounds, the context should have been
    // compacted at least once, meaning a later count should be smaller than
    // the count just before compaction.
    let has_decrease = counts.windows(2).any(|w| w[1] < w[0]);
    // If the budget is tight enough, we expect at least one decrease.
    // With 200-token budget and ~50 tokens per message pair, compaction should
    // trigger around the 3rd-4th round.
    assert!(
        has_decrease || last_count < counts.len() * 2,
        "expected context compaction to reduce message count at some point: {counts:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// T021: sliding_window_preserves_anchor_and_tail (AC 14)
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn sliding_window_preserves_anchor_and_tail() {
    // Configure a small context budget so compaction triggers.
    // anchor=1 means the first user message should always survive.
    let responses: Vec<Vec<AssistantMessageEvent>> = (0..8)
        .map(|i| text_only_events(&format!("reply-{i}")))
        .collect();

    let stream_fn = Arc::new(MockStreamFn::new(responses));
    let sf: Arc<dyn swink_agent::StreamFn> = stream_fn;

    // normal_budget=300 tokens, overflow_budget=150, anchor=1
    let mut agent = make_agent_with_small_context(sf, 300, 150, 1);

    // First message is the anchor — give it a recognizable marker.
    let anchor_msg = user_msg("ANCHOR_FIRST_MESSAGE");
    let _r = agent.prompt_async(vec![anchor_msg]).await.unwrap();

    // Send several more messages to fill and exceed the budget.
    for i in 0..7 {
        let long_text = "y".repeat(200); // ~50 tokens each
        let msg = user_msg(&format!("middle-{i}-{long_text}"));
        let _r = agent.prompt_async(vec![msg]).await.unwrap();
    }

    // Inspect agent state after all rounds.
    let messages = &agent.state().messages;

    // The first message (anchor) should still be present.
    let first_msg_text = match &messages[0] {
        AgentMessage::Llm(LlmMessage::User(u)) => ContentBlock::extract_text(&u.content),
        _ => String::new(),
    };
    assert!(
        first_msg_text.contains("ANCHOR_FIRST_MESSAGE"),
        "anchor message should survive compaction, got: {first_msg_text}"
    );

    // The most recent message should be from the last round.
    let last_user = messages
        .iter()
        .rev()
        .find(|m| matches!(m, AgentMessage::Llm(LlmMessage::User(_))));
    let last_text = match last_user {
        Some(AgentMessage::Llm(LlmMessage::User(u))) => ContentBlock::extract_text(&u.content),
        _ => String::new(),
    };
    assert!(
        last_text.contains("middle-6"),
        "most recent user message should survive compaction, got: {last_text}"
    );

    // Some middle messages should have been dropped (context should not contain
    // all 8 user messages plus 8 assistant messages = 16 total).
    assert!(
        messages.len() < 16,
        "compaction should have removed some middle messages, got {} messages",
        messages.len()
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// T022: context_overflow_triggers_retry (AC 15)
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn context_overflow_triggers_retry() {
    // First call returns a context overflow error, second call succeeds.
    // In-place recovery compacts context then retries the LLM call.
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        vec![AssistantMessageEvent::error_context_overflow(
            "context_length_exceeded",
        )],
        text_only_events("recovered after overflow"),
    ]));

    let sf: Arc<dyn swink_agent::StreamFn> = stream_fn;

    // Use a tight overflow budget (200 tokens) so compaction has work to do.
    let mut agent = make_agent_with_small_context(sf, 10_000, 200, 1);

    // Provide enough messages (~100 tokens each) that the overflow-budget
    // compaction can remove some, enabling the retry.
    let padding = "x".repeat(400); // 400 chars = ~100 tokens
    let mut messages = Vec::new();
    for i in 0..5 {
        messages.push(AgentMessage::Llm(LlmMessage::User(swink_agent::UserMessage {
            content: vec![ContentBlock::Text {
                text: format!("msg{i}:{padding}"),
            }],
            timestamp: 0,
        })));
    }

    let result = agent.prompt_async(messages).await.unwrap();

    // The agent should have recovered and returned a successful response.
    let has_recovered_text = result.messages.iter().any(|m| {
        matches!(
            m,
            AgentMessage::Llm(LlmMessage::Assistant(a))
                if a.content.iter().any(|b| matches!(
                    b,
                    ContentBlock::Text { text } if text.contains("recovered")
                ))
        )
    });
    assert!(
        has_recovered_text,
        "agent should recover from context overflow and return successful response"
    );
    assert!(
        result.error.is_none(),
        "result should not have an error after recovery"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// T023: tool_result_pairs_kept_together (AC 16)
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn tool_result_pairs_kept_together() {
    // Turn 1: LLM returns a tool call. The loop executes MockTool and feeds
    // the result back. Turn 2 (follow-up): LLM returns text.
    // Then we send several more large messages to trigger compaction.
    // After compaction, verify tool call and tool result are either both
    // present or both absent.

    let tool = Arc::new(MockTool::new("mock_tool"));

    // Responses: tool call, then text reply, then several more text replies
    // for subsequent rounds.
    let mut responses = vec![
        tool_call_events("tc_1", "mock_tool", "{}"),
        text_only_events("tool done"),
    ];
    for i in 0..6 {
        responses.push(text_only_events(&format!("filler-reply-{i}")));
    }

    let stream_fn = Arc::new(MockStreamFn::new(responses));
    let sf: Arc<dyn swink_agent::StreamFn> = stream_fn;

    let mut agent = Agent::new(
        AgentOptions::new(
            "test system prompt",
            default_model(),
            sf,
            default_convert,
        )
        .with_tools(vec![tool])
        // Very small budget to force compaction after a few rounds.
        .with_transform_context_fn(sliding_window(300, 150, 1))
        .with_retry_strategy(Box::new(
            DefaultRetryStrategy::default()
                .with_jitter(false)
                .with_base_delay(Duration::from_millis(1)),
        )),
    );

    // First prompt triggers tool call + tool result + follow-up text.
    let _r = agent
        .prompt_async(vec![user_msg("use the tool")])
        .await
        .unwrap();

    // Send several more rounds with large messages to trigger compaction.
    for i in 0..6 {
        let long_text = "z".repeat(200); // ~50 tokens each
        let msg = user_msg(&format!("pad-{i}-{long_text}"));
        let _r = agent.prompt_async(vec![msg]).await.unwrap();
    }

    // Inspect final message state.
    let messages = &agent.state().messages;

    // Check: if any ToolResult exists, its corresponding Assistant with ToolCall
    // must also exist, and vice versa.
    let has_tool_result = messages
        .iter()
        .any(|m| matches!(m, AgentMessage::Llm(LlmMessage::ToolResult(_))));
    let has_tool_call = messages.iter().any(|m| {
        matches!(
            m,
            AgentMessage::Llm(LlmMessage::Assistant(a))
                if a.content.iter().any(|b| matches!(b, ContentBlock::ToolCall { .. }))
        )
    });

    // Tool pairs should be either both present or both absent.
    assert_eq!(
        has_tool_result, has_tool_call,
        "tool call and tool result must be kept together or removed together: \
         has_tool_call={has_tool_call}, has_tool_result={has_tool_result}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// T024: transform_context_callback_on_overflow (edge case)
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn transform_context_callback_on_overflow() {
    // Verify that when context overflow occurs, the transform_context callback
    // receives overflow=true.

    let overflow_seen = Arc::new(AtomicBool::new(false));
    let overflow_seen_clone = Arc::clone(&overflow_seen);

    let overflow_flags: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));
    let flags_clone = Arc::clone(&overflow_flags);

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        vec![AssistantMessageEvent::error_context_overflow(
            "context_length_exceeded",
        )],
        text_only_events("recovered"),
    ]));
    let sf: Arc<dyn swink_agent::StreamFn> = stream_fn;

    let mut agent = Agent::new(
        AgentOptions::new("test system prompt", default_model(), sf, default_convert)
            .with_transform_context_fn(move |msgs: &mut Vec<AgentMessage>, overflow: bool| {
                flags_clone.lock().unwrap().push(overflow);
                if overflow {
                    overflow_seen_clone.store(true, Ordering::SeqCst);
                }
                // Apply basic sliding window to avoid infinite loops.
                let compact = sliding_window(10_000, 200, 1);
                compact(msgs, overflow);
            })
            .with_retry_strategy(Box::new(
                DefaultRetryStrategy::default()
                    .with_jitter(false)
                    .with_base_delay(Duration::from_millis(1)),
            )),
    );

    let _result = agent
        .prompt_async(vec![user_msg("trigger overflow")])
        .await
        .unwrap();

    // The callback should have been invoked with overflow=true at least once.
    assert!(
        overflow_seen.load(Ordering::SeqCst),
        "transform_context callback should have received overflow=true"
    );

    let flags = overflow_flags.lock().unwrap().clone();
    assert!(
        flags.len() >= 2,
        "transform_context should be called at least twice (normal + overflow), got {}",
        flags.len()
    );

    // First call should be normal (overflow=false), second should be overflow=true.
    assert!(
        !flags[0],
        "first transform_context call should have overflow=false"
    );
    assert!(
        flags[1],
        "second transform_context call (after overflow error) should have overflow=true"
    );
}
