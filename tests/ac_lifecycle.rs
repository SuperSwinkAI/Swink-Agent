//! Integration tests for User Story 1: Core Agent Lifecycle and Events.
//!
//! Tests T004-T010 covering agent creation, message processing, lifecycle
//! event ordering, streaming token delivery, history accumulation, and
//! panicking subscriber removal.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{
    EventCollector, MockStreamFn, default_convert, default_model, text_only_events, user_msg,
};

use swink_agent::{
    Agent, AgentEvent, AgentMessage, AgentOptions, AssistantMessageEvent, ContentBlock, Cost,
    DefaultRetryStrategy, LlmMessage, StopReason, Usage,
};

// ─── Helpers ─────────────────────────────────────────────────────────────

fn make_agent(stream_fn: Arc<dyn swink_agent::StreamFn>) -> Agent {
    Agent::new(
        AgentOptions::new("test prompt", default_model(), stream_fn, default_convert)
            .with_retry_strategy(Box::new(
                DefaultRetryStrategy::default()
                    .with_jitter(false)
                    .with_base_delay(Duration::from_millis(1)),
            )),
    )
}

// ═════════════════════════════════════════════════════════════════════════
// T004 — AC 1: Agent creation with mock stream
// ═════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn agent_creation_with_mock_stream() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events(
        "Hello from agent",
    )]));
    let mut agent = make_agent(stream_fn);

    let result = agent.prompt_async(vec![user_msg("hi")]).await.unwrap();

    // Should produce at least one assistant message with text content.
    let has_assistant_text = result.messages.iter().any(|m| {
        matches!(
            m,
            AgentMessage::Llm(LlmMessage::Assistant(a))
                if a.content.iter().any(|b| matches!(b, ContentBlock::Text { text } if !text.is_empty()))
        )
    });
    assert!(
        has_assistant_text,
        "prompt_async should return a text response from the mock stream"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// T005 — AC 2: Message processing produces correct response
// ═════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn message_processing_produces_response() {
    let expected_text = "The answer is 42";
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events(expected_text)]));
    let mut agent = make_agent(stream_fn);

    let result = agent
        .prompt_async(vec![user_msg("What is the answer?")])
        .await
        .unwrap();

    // Extract the assistant text and verify it matches the scripted output.
    let response_text: String = result
        .messages
        .iter()
        .filter_map(|m| match m {
            AgentMessage::Llm(LlmMessage::Assistant(a)) => Some(
                a.content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(""),
            ),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");

    assert_eq!(
        response_text, expected_text,
        "response text should match the scripted mock output"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// T006 — AC 3: Lifecycle events emitted in order
// ═════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn lifecycle_events_emitted_in_order() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let mut agent = make_agent(stream_fn);

    let collector = EventCollector::new();
    let _sub = agent.subscribe(collector.subscriber());

    let _result = agent.prompt_async(vec![user_msg("hi")]).await.unwrap();

    let turn_start = collector
        .position("TurnStart")
        .expect("TurnStart must be emitted");
    let msg_start = collector
        .position("MessageStart")
        .expect("MessageStart must be emitted");
    let msg_end = collector
        .position("MessageEnd")
        .expect("MessageEnd must be emitted");
    let turn_end = collector
        .position("TurnEnd")
        .expect("TurnEnd must be emitted");

    assert!(
        turn_start < msg_start,
        "TurnStart ({turn_start}) must precede MessageStart ({msg_start})"
    );
    assert!(
        msg_start < msg_end,
        "MessageStart ({msg_start}) must precede MessageEnd ({msg_end})"
    );
    assert!(
        msg_end < turn_end,
        "MessageEnd ({msg_end}) must precede TurnEnd ({turn_end})"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// T007 — AC 4: Streaming delivers text tokens via MessageUpdate
// ═════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn streaming_delivers_text_tokens() {
    // Build a multi-delta response to verify individual token delivery.
    let multi_token_events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::TextStart { content_index: 0 },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "Hel".to_string(),
        },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "lo ".to_string(),
        },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "World".to_string(),
        },
        AssistantMessageEvent::TextEnd { content_index: 0 },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            usage: Usage::default(),
            cost: Cost::default(),
        },
    ];

    let stream_fn = Arc::new(MockStreamFn::new(vec![multi_token_events]));
    let mut agent = make_agent(stream_fn);

    let collector = EventCollector::new();
    let _sub = agent.subscribe(collector.subscriber());

    let result = agent
        .prompt_async(vec![user_msg("say hello")])
        .await
        .unwrap();

    // Verify MessageUpdate events were emitted.
    let update_count = collector
        .events()
        .iter()
        .filter(|n| n.as_str() == "MessageUpdate")
        .count();
    assert!(
        update_count >= 3,
        "expected at least 3 MessageUpdate events for 3 deltas, got {update_count}"
    );

    // Verify the accumulated text is the concatenation of all deltas.
    let full_text: String = result
        .messages
        .iter()
        .filter_map(|m| match m {
            AgentMessage::Llm(LlmMessage::Assistant(a)) => Some(
                a.content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(""),
            ),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");

    assert_eq!(
        full_text, "Hello World",
        "accumulated text should be the concatenation of all deltas"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// T008 — AC 5: Turn completion accumulates history
// ═════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn turn_completion_accumulates_history() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        text_only_events("first response"),
        text_only_events("second response"),
    ]));
    let mut agent = make_agent(stream_fn);

    // First prompt.
    let _result1 = agent
        .prompt_async(vec![user_msg("first question")])
        .await
        .unwrap();

    // Second prompt.
    let _result2 = agent
        .prompt_async(vec![user_msg("second question")])
        .await
        .unwrap();

    // After two prompt_async calls, the agent state should contain all messages:
    // 2 user messages + 2 assistant messages = 4 total.
    let messages = &agent.state().messages;

    let user_count = messages
        .iter()
        .filter(|m| matches!(m, AgentMessage::Llm(LlmMessage::User(_))))
        .count();
    let assistant_count = messages
        .iter()
        .filter(|m| matches!(m, AgentMessage::Llm(LlmMessage::Assistant(_))))
        .count();

    assert_eq!(
        user_count, 2,
        "agent state should contain 2 user messages, got {user_count}"
    );
    assert_eq!(
        assistant_count, 2,
        "agent state should contain 2 assistant messages, got {assistant_count}"
    );
    assert_eq!(
        messages.len(),
        4,
        "agent state should contain 4 total messages (2 user + 2 assistant), got {}",
        messages.len()
    );
}

// ═════════════════════════════════════════════════════════════════════════
// T009/T010 — Edge case: Panicking subscriber is removed
// ═════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn panicking_subscriber_is_removed() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let mut agent = make_agent(stream_fn);

    // Register a subscriber that panics on every event.
    agent.subscribe(|_event: &AgentEvent| {
        panic!("intentional panic");
    });

    // Register a second, well-behaved subscriber.
    let collector = EventCollector::new();
    let _sub = agent.subscribe(collector.subscriber());

    // Run the agent. The panicking subscriber should be caught and removed,
    // allowing the second subscriber to receive events normally.
    let result = agent.prompt_async(vec![user_msg("hi")]).await.unwrap();

    // The second subscriber should have received events despite the first panicking.
    assert!(
        collector.count() > 0,
        "second subscriber should receive events even when first subscriber panics"
    );

    // The agent should still produce a valid result.
    let has_assistant = result
        .messages
        .iter()
        .any(|m| matches!(m, AgentMessage::Llm(LlmMessage::Assistant(_))));
    assert!(
        has_assistant,
        "agent should produce a valid assistant response despite panicking subscriber"
    );
}
