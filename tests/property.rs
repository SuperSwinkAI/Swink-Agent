//! Property-based tests using proptest.

use proptest::prelude::*;
use std::time::Duration;
use swink_agent::{
    AgentMessage, AssistantMessageEvent, ContentBlock, Cost, DefaultRetryStrategy, LlmMessage,
    RetryStrategy, StopReason, Usage, UserMessage, accumulate_message, estimate_tokens,
    sliding_window,
};

// ─── Helpers ────────────────────────────────────────────────────────────────

fn text_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: text.to_owned(),
        }],
        timestamp: 0,
    }))
}

/// Strategy that produces a Vec of text lengths, which we materialise into
/// `AgentMessage` values at test time (avoids the `Clone` bound on `AgentMessage`).
fn arb_message_lengths(max_len: usize) -> impl Strategy<Value = Vec<usize>> {
    proptest::collection::vec(4_usize..2000, 1..=max_len)
}

fn lengths_to_messages(lengths: &[usize]) -> Vec<AgentMessage> {
    lengths
        .iter()
        .map(|&n| text_message(&"x".repeat(n)))
        .collect()
}

// ─── Test 1: compact_sliding_window — tokens ≤ budget after compaction ──────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn compacted_tokens_within_budget(
        lengths in arb_message_lengths(20),
        budget in 50_usize..5000,
        anchor in 0_usize..5,
    ) {
        let compact = sliding_window(budget, budget / 2, anchor);
        let mut msgs = lengths_to_messages(&lengths);
        let total_before: usize = msgs.iter().map(|m| estimate_tokens(m)).sum();
        compact(&mut msgs, false);
        let total_after: usize = msgs.iter().map(|m| estimate_tokens(m)).sum();

        // After compaction, tokens should be ≤ budget, unless:
        // - already under budget (no compaction needed), or
        // - anchor messages alone exceed the budget (anchors always preserved)
        let anchor_tokens: usize = msgs.iter().take(anchor).map(|m| estimate_tokens(m)).sum();
        prop_assert!(
            total_after <= budget || total_before <= budget || total_after <= anchor_tokens,
            "total_after={total_after} budget={budget} anchor_tokens={anchor_tokens} total_before={total_before}"
        );
    }

    #[test]
    fn compaction_preserves_anchor_messages(
        lengths in arb_message_lengths(15),
        budget in 10_usize..2000,
        anchor in 1_usize..4,
    ) {
        let compact = sliding_window(budget, budget / 2, anchor);
        let original = lengths_to_messages(&lengths);
        let mut msgs = lengths_to_messages(&lengths);
        let effective_anchor = anchor.min(msgs.len());

        compact(&mut msgs, false);

        // Anchor messages must always be preserved at the front.
        prop_assert!(msgs.len() >= effective_anchor);
        for i in 0..effective_anchor {
            let orig_tokens = estimate_tokens(&original[i]);
            let kept_tokens = estimate_tokens(&msgs[i]);
            prop_assert_eq!(orig_tokens, kept_tokens, "anchor message {} was altered", i);
        }
    }

    #[test]
    fn compaction_never_increases_message_count(
        lengths in arb_message_lengths(20),
        budget in 10_usize..5000,
        anchor in 0_usize..5,
    ) {
        let compact = sliding_window(budget, budget / 2, anchor);
        let original_len = lengths.len();
        let mut msgs = lengths_to_messages(&lengths);
        compact(&mut msgs, false);
        prop_assert!(msgs.len() <= original_len);
    }
}

// ─── Test 2: Streaming accumulator — ordering invariants ────────────────────

/// Strategy that generates a valid sequence of stream events with N text blocks.
fn arb_valid_stream(
    max_blocks: usize,
) -> impl Strategy<Value = (Vec<AssistantMessageEvent>, usize)> {
    (1..=max_blocks).prop_flat_map(|n_blocks| {
        proptest::collection::vec(
            proptest::string::string_regex("[a-zA-Z0-9 ]{1,100}").unwrap(),
            n_blocks..=n_blocks,
        )
        .prop_map(move |texts: Vec<String>| {
            let mut events = vec![AssistantMessageEvent::Start];
            for (i, text) in texts.iter().enumerate() {
                events.push(AssistantMessageEvent::TextStart { content_index: i });
                events.push(AssistantMessageEvent::TextDelta {
                    content_index: i,
                    delta: text.clone(),
                });
                events.push(AssistantMessageEvent::TextEnd { content_index: i });
            }
            events.push(AssistantMessageEvent::Done {
                stop_reason: StopReason::Stop,
                usage: Usage::default(),
                cost: Cost::default(),
            });
            (events, n_blocks)
        })
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn accumulator_produces_correct_block_count(
        (events, expected_blocks) in arb_valid_stream(8),
    ) {
        let result = accumulate_message(events, "test-provider", "test-model");
        let msg = result.expect("valid event sequence should produce a message");
        prop_assert_eq!(msg.content.len(), expected_blocks);
    }

    #[test]
    fn accumulator_text_content_matches_deltas(
        (events, _) in arb_valid_stream(5),
    ) {
        // Collect the expected text per block from the delta events.
        let mut expected: Vec<String> = Vec::new();
        for event in &events {
            if let AssistantMessageEvent::TextStart { .. } = event {
                expected.push(String::new());
            }
            if let AssistantMessageEvent::TextDelta { delta, content_index } = event {
                if let Some(s) = expected.get_mut(*content_index) {
                    s.push_str(delta);
                }
            }
        }

        let msg = accumulate_message(events, "test", "test").expect("should succeed");
        for (i, block) in msg.content.iter().enumerate() {
            if let ContentBlock::Text { text } = block {
                prop_assert_eq!(text, &expected[i], "text mismatch at block {}", i);
            }
        }
    }
}

// ─── Test 3: Retry jitter — output within [0.5, 1.5) × delay ───────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    #[test]
    fn retry_jitter_within_bounds(
        base_ms in 1_u64..10_000,
        multiplier in 1.0_f64..5.0,
        attempt in 1_u32..8,
    ) {
        let strategy = DefaultRetryStrategy {
            max_attempts: 10,
            base_delay: Duration::from_millis(base_ms),
            max_delay: Duration::from_secs(3600),
            multiplier,
            jitter: true,
        };

        let delay = strategy.delay(attempt);

        // Compute the expected base (before jitter): base_delay * multiplier^(attempt-1)
        let exp = multiplier.powi(attempt.saturating_sub(1) as i32);
        let base_secs = (base_ms as f64 / 1000.0) * exp;
        let capped_secs = base_secs.min(3600.0);

        // Jitter factor is in [0.5, 1.5), so result should be in [0.5 * capped, 1.5 * capped).
        let lower = capped_secs * 0.5;
        let upper = capped_secs * 1.5;
        let actual = delay.as_secs_f64();

        prop_assert!(
            actual >= lower - 1e-9 && actual < upper + 1e-9,
            "delay {actual}s not in [{lower}, {upper}) for base_ms={base_ms} mult={multiplier} attempt={attempt}"
        );
    }

    #[test]
    fn retry_no_jitter_is_deterministic(
        base_ms in 1_u64..10_000,
        multiplier in 1.0_f64..5.0,
        attempt in 1_u32..8,
    ) {
        let strategy = DefaultRetryStrategy {
            max_attempts: 10,
            base_delay: Duration::from_millis(base_ms),
            max_delay: Duration::from_secs(3600),
            multiplier,
            jitter: false,
        };

        let d1 = strategy.delay(attempt);
        let d2 = strategy.delay(attempt);
        prop_assert_eq!(d1, d2, "no-jitter delay should be deterministic");
    }

    #[test]
    fn retry_delay_capped_at_max(
        base_ms in 100_u64..5000,
        max_ms in 100_u64..5000,
        attempt in 1_u32..10,
    ) {
        let strategy = DefaultRetryStrategy {
            max_attempts: 10,
            base_delay: Duration::from_millis(base_ms),
            max_delay: Duration::from_millis(max_ms),
            multiplier: 2.0,
            jitter: false,
        };

        let delay = strategy.delay(attempt);
        // Without jitter, delay should never exceed max_delay.
        prop_assert!(
            delay <= Duration::from_millis(max_ms),
            "delay {:?} exceeded max {:?}",
            delay,
            Duration::from_millis(max_ms)
        );
    }
}
