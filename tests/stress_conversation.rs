#![cfg(feature = "testkit")]
//! Stress test: 100+ turn conversation with context compaction.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use swink_agent::{Agent, AgentEvent, AgentOptions, SlidingWindowTransformer, from_fns};

use common::{default_convert, default_model, text_events, user_msg};

const TURN_COUNT: usize = 110;

#[tokio::test]
async fn many_turns_with_compaction() {
    // Build scripted responses: one text reply per turn.
    let responses: Vec<Vec<_>> = (0..TURN_COUNT)
        .map(|i| text_events(&format!("reply {i}")))
        .collect();

    let stream_fn = Arc::new(common::MockStreamFn::new(responses));

    // Use a very small token budget so compaction fires frequently.
    let transformer = SlidingWindowTransformer::new(
        200, // normal_budget — very small to force compaction
        100, // overflow_budget
        1,   // anchor — preserve only the first message
    );

    // Pre-build all follow-up messages.
    let follow_ups: Arc<Mutex<Vec<swink_agent::AgentMessage>>> = Arc::new(Mutex::new(
        (1..TURN_COUNT)
            .rev()
            .map(|i| user_msg(&format!("follow-up {i}")))
            .collect(),
    ));

    // Create a message provider that drip-feeds one follow-up per poll.
    let follow_ups_clone = Arc::clone(&follow_ups);
    let provider = from_fns(
        Vec::new, // no steering messages
        move || {
            let mut guard = follow_ups_clone.lock().unwrap();
            guard.pop().map_or_else(Vec::new, |msg| vec![msg])
        },
    );

    let opts = AgentOptions::new(
        "You are a helpful assistant.",
        default_model(),
        stream_fn,
        default_convert,
    )
    .with_transform_context(transformer)
    .with_external_message_provider(provider);

    let mut agent = Agent::new(opts);

    // Track events.
    let turn_start_count = Arc::new(AtomicUsize::new(0));
    let compacted_count = Arc::new(AtomicUsize::new(0));
    let turn_start_clone = Arc::clone(&turn_start_count);
    let compacted_clone = Arc::clone(&compacted_count);

    agent.subscribe(move |event: &AgentEvent| match event {
        AgentEvent::TurnStart => {
            turn_start_clone.fetch_add(1, Ordering::SeqCst);
        }
        AgentEvent::ContextCompacted { .. } => {
            compacted_clone.fetch_add(1, Ordering::SeqCst);
        }
        _ => {}
    });

    // Run with initial message and a generous timeout.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        agent.prompt_async(vec![user_msg("start")]),
    )
    .await;

    assert!(result.is_ok(), "agent timed out after 30s");
    let agent_result = result.unwrap();
    assert!(
        agent_result.is_ok(),
        "agent returned error: {:?}",
        agent_result.err()
    );

    let turns = turn_start_count.load(Ordering::SeqCst);
    assert!(
        turns >= TURN_COUNT,
        "expected at least {TURN_COUNT} TurnStart events, got {turns}"
    );

    let compactions = compacted_count.load(Ordering::SeqCst);
    assert!(
        compactions >= 1,
        "expected at least one ContextCompacted event, got {compactions}"
    );
}
