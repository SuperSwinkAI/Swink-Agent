//! Integration tests for User Story 5: Structured Output and Proxy Reconstruction.
//! Tests T033-T037.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{MockStreamFn, default_convert, default_model, text_only_events, tool_call_events};
use serde_json::json;

use swink_agent::{
    Agent, AgentOptions, AssistantMessageEvent, ContentBlock, Cost, DefaultRetryStrategy,
    StopReason, Usage, accumulate_message,
};

// ─── Helpers ─────────────────────────────────────────────────────────────

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

// ─── T033 (AC 23): structured_output_with_schema ─────────────────────────

#[tokio::test]
async fn structured_output_with_schema() {
    let schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" }
        },
        "required": ["name"]
    });

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("so_1", "__structured_output", r#"{"name":"Alice"}"#),
        text_only_events("done"),
    ]));
    let mut agent = make_agent(stream_fn);

    let value = agent
        .structured_output("Extract the name".to_string(), schema)
        .await
        .unwrap();

    assert_eq!(value["name"], "Alice");
}

// ─── T034 (AC 24): schema_enforcement_rejects_invalid ────────────────────

#[tokio::test]
async fn schema_enforcement_rejects_invalid() {
    let schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" }
        },
        "required": ["name"]
    });

    // 2 failed attempts (invalid response) + 1 successful attempt = 3 attempts.
    // Each attempt needs: tool_call_events + text_only_events = 6 response sequences.
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        // Attempt 0: invalid — missing required "name"
        tool_call_events("so_1", "__structured_output", r#"{"wrong": 1}"#),
        text_only_events("done"),
        // Attempt 1 (retry): invalid again
        tool_call_events("so_2", "__structured_output", r#"{"wrong": 2}"#),
        text_only_events("done"),
        // Attempt 2 (retry): valid
        tool_call_events("so_3", "__structured_output", r#"{"name":"Bob"}"#),
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

// ─── T035 (AC 25): proxy_stream_reconstruction ──────────────────────────

#[tokio::test]
async fn proxy_stream_reconstruction() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::TextStart { content_index: 0 },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "hello world".to_string(),
        },
        AssistantMessageEvent::TextEnd { content_index: 0 },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            usage: Usage::default(),
            cost: Cost::default(),
        },
    ];

    let msg = accumulate_message(events, "proxy", "test-model").unwrap();

    assert_eq!(msg.content.len(), 1);
    assert!(
        matches!(&msg.content[0], ContentBlock::Text { text } if text == "hello world"),
        "expected Text content block with 'hello world', got {:?}",
        msg.content[0]
    );
    assert_eq!(msg.stop_reason, StopReason::Stop);
}

// ─── T036: structured_output_empty_object (edge case) ────────────────────

#[tokio::test]
async fn structured_output_empty_object() {
    let schema = json!({ "type": "object" });

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("so_1", "__structured_output", "{}"),
        text_only_events("done"),
    ]));
    let mut agent = make_agent(stream_fn);

    let value = agent
        .structured_output("Return empty object".to_string(), schema)
        .await
        .unwrap();

    assert_eq!(value, json!({}));
}
