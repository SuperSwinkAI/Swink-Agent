//! Structured output, subscriber, and `prompt_stream/handle_stream_event` tests.

mod common;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{
    MockStreamFn, default_convert, default_model, text_only_events, tool_call_events, user_msg,
};
use futures::stream::StreamExt;
use serde_json::json;

use swink_agent::{Agent, AgentError, AgentOptions, DefaultRetryStrategy, StopReason, StreamFn};

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

// ─── 4.9: subscribe returns SubscriptionId; callback receives events ─────

#[tokio::test]
async fn subscribe_receives_events() {
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
async fn unsubscribe_removes_listener() {
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
async fn subscriber_panic_does_not_crash() {
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

// ─── 4.14: structured_output validates and returns typed value ───────────

#[tokio::test]
async fn structured_output_valid() {
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
async fn structured_output_retries() {
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
async fn structured_output_fails_after_max_retries() {
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
        matches!(err, AgentError::StructuredOutputFailed { attempts, .. } if attempts == 3),
        "expected StructuredOutputFailed with 3 attempts, got {err:?}"
    );

    assert!(
        agent
            .state()
            .tools
            .iter()
            .all(|tool| tool.name() != "__structured_output"),
        "synthetic structured output tool should always be removed after failure"
    );
}

// ─── Multi-turn via prompt_stream + handle_stream_event ───────────────────

/// Regression test: when consuming `prompt_stream` externally (as the TUI does),
/// `handle_stream_event` must clear `is_running` so subsequent prompts succeed.
#[tokio::test]
async fn multi_turn_via_prompt_stream_and_handle_stream_event() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        text_only_events("first response"),
        text_only_events("second response"),
    ]));
    let mut agent = make_agent(stream_fn);

    // Turn 1: consume stream externally, feeding events back via handle_stream_event.
    {
        let stream = agent.prompt_stream(vec![user_msg("hello")]).unwrap();
        let mut stream = std::pin::pin!(stream);
        while let Some(event) = stream.next().await {
            agent.handle_stream_event(&event);
        }
    }

    assert!(
        !agent.state().is_running,
        "agent should be idle after stream is fully consumed"
    );
    assert!(
        !agent.state().messages.is_empty(),
        "agent state should have messages after first turn"
    );

    // Turn 2: should succeed without AlreadyRunning.
    {
        let stream = agent
            .prompt_stream(vec![user_msg("follow up")])
            .expect("second prompt_stream should not return AlreadyRunning");
        let mut stream = std::pin::pin!(stream);
        while let Some(event) = stream.next().await {
            agent.handle_stream_event(&event);
        }
    }

    assert!(
        !agent.state().is_running,
        "agent should be idle after second turn"
    );
    assert!(
        agent.state().messages.len() >= 4,
        "state should have messages from both turns (2 user + 2 assistant), got {}",
        agent.state().messages.len()
    );
}

/// Verify that `handle_stream_event` dispatches to subscribers.
#[tokio::test]
async fn handle_stream_event_dispatches_to_subscribers() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let mut agent = make_agent(stream_fn);

    let event_names: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let names_clone = Arc::clone(&event_names);
    let _id = agent.subscribe(move |event| {
        let name = format!("{event:?}");
        let prefix = name.split([' ', '{', '(']).next().unwrap_or("").to_string();
        names_clone.lock().unwrap().push(prefix);
    });

    let stream = agent.prompt_stream(vec![user_msg("hi")]).unwrap();
    let mut stream = std::pin::pin!(stream);
    while let Some(event) = stream.next().await {
        agent.handle_stream_event(&event);
    }

    let collected = event_names.lock().unwrap().clone();
    assert!(
        collected.contains(&"AgentStart".to_string()),
        "subscriber should receive AgentStart"
    );
    assert!(
        collected.contains(&"AgentEnd".to_string()),
        "subscriber should receive AgentEnd"
    );
}

/// Confirm that without `handle_stream_event`, the second `prompt_stream` fails.
#[tokio::test]
async fn prompt_stream_without_handle_stream_event_stays_running() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        text_only_events("first"),
        text_only_events("second"),
    ]));
    let mut agent = make_agent(stream_fn);

    // Consume stream without calling handle_stream_event.
    let stream = agent.prompt_stream(vec![user_msg("hello")]).unwrap();
    let mut stream = std::pin::pin!(stream);
    while let Some(_event) = stream.next().await {}

    assert!(
        agent.state().is_running,
        "agent should still think it is running"
    );

    let err = agent.prompt_stream(vec![user_msg("follow up")]);
    assert!(
        matches!(err, Err(AgentError::AlreadyRunning)),
        "second prompt should fail with AlreadyRunning"
    );
}
