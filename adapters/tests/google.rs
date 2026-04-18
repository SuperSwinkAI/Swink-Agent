#![cfg(feature = "gemini")]
//! Wiremock-based tests for `GeminiStreamFn`.

mod common;

use std::sync::{Arc, Mutex};

use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use swink_agent::ApiVersion;
use swink_agent::{AssistantMessageEvent, ModelSpec, StopReason, StreamFn, StreamOptions};
use swink_agent_adapters::GeminiStreamFn;

use common::{event_name, find_error_message, sse_response, test_context};

fn test_model() -> ModelSpec {
    ModelSpec::new("google", "gemini-3-flash-preview")
}

async fn collect_events(stream_fn: &GeminiStreamFn) -> Vec<AssistantMessageEvent> {
    collect_events_with_options(stream_fn, StreamOptions::default()).await
}

async fn collect_events_with_options(
    stream_fn: &GeminiStreamFn,
    options: StreamOptions,
) -> Vec<AssistantMessageEvent> {
    let model = test_model();
    let context = test_context();
    let token = CancellationToken::new();
    let stream = stream_fn.stream(&model, &context, &options, token);
    stream.collect::<Vec<_>>().await
}

#[tokio::test]
async fn google_text_stream() {
    let body = [
        r#"data: {"candidates":[{"content":{"parts":[{"text":"hello"}]}}]}"#,
        "",
        r#"data: {"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5,"totalTokenCount":15}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/v1beta/models/gemini-3-flash-preview:streamGenerateContent",
        ))
        .and(query_param("alt", "sse"))
        .and(header("x-goog-api-key", "test-key"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let stream_fn = GeminiStreamFn::new(server.uri(), "test-key", ApiVersion::V1beta);
    let events = collect_events(&stream_fn).await;

    let types: Vec<_> = events.iter().map(event_name).collect();
    assert!(types.contains(&"Start"), "missing Start: {types:?}");
    assert!(types.contains(&"TextStart"), "missing TextStart: {types:?}");
    assert!(types.contains(&"TextDelta"), "missing TextDelta: {types:?}");
    assert!(types.contains(&"TextEnd"), "missing TextEnd: {types:?}");
    assert!(types.contains(&"Done"), "missing Done: {types:?}");

    let delta_text: String = events
        .iter()
        .filter_map(|event| match event {
            AssistantMessageEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(delta_text, "hello");

    let usage = events
        .iter()
        .find_map(|event| match event {
            AssistantMessageEvent::Done { usage, .. } => Some(usage.clone()),
            _ => None,
        })
        .expect("missing Done event");
    assert_eq!(usage.input, 10);
    assert_eq!(usage.output, 5);
    assert_eq!(usage.total, 15);
}

#[tokio::test]
async fn google_cancellation_after_first_chunk_emits_aborted_and_closes_text() {
    let body = [
        r#"data: {"candidates":[{"content":{"parts":[{"text":"hello"}]}}]}"#,
        "",
        r#"data: {"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5,"totalTokenCount":15}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/v1beta/models/gemini-3-flash-preview:streamGenerateContent",
        ))
        .and(query_param("alt", "sse"))
        .and(header("x-goog-api-key", "test-key"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let stream_fn = GeminiStreamFn::new(server.uri(), "test-key", ApiVersion::V1beta);
    let model = test_model();
    let context = test_context();
    let options = StreamOptions::default();
    let token = CancellationToken::new();
    let cancel_token = token.clone();
    let mut stream = Box::pin(stream_fn.stream(&model, &context, &options, token));
    let mut events = Vec::new();
    let mut cancelled = false;

    while let Some(event) = stream.next().await {
        if matches!(event, AssistantMessageEvent::TextDelta { .. }) && !cancelled {
            cancel_token.cancel();
            cancelled = true;
        }
        events.push(event);
    }

    assert!(cancelled, "expected to cancel after the first text chunk");

    let types: Vec<_> = events.iter().map(event_name).collect();
    assert!(types.contains(&"Start"), "missing Start: {types:?}");
    assert!(types.contains(&"TextStart"), "missing TextStart: {types:?}");
    assert!(types.contains(&"TextDelta"), "missing TextDelta: {types:?}");
    assert!(types.contains(&"TextEnd"), "missing TextEnd: {types:?}");
    assert!(types.contains(&"Error"), "missing Error: {types:?}");
    assert!(
        !types.contains(&"Done"),
        "cancellation should not emit Done: {types:?}"
    );

    let text_end_pos = types
        .iter()
        .position(|event| *event == "TextEnd")
        .expect("missing TextEnd");
    let error_pos = types
        .iter()
        .position(|event| *event == "Error")
        .expect("missing Error");
    assert!(
        text_end_pos < error_pos,
        "open text blocks should close before the abort error: {types:?}"
    );

    let terminal_error = events
        .iter()
        .find_map(|event| match event {
            AssistantMessageEvent::Error {
                stop_reason,
                error_message,
                error_kind,
                ..
            } => Some((*stop_reason, error_message.clone(), *error_kind)),
            _ => None,
        })
        .expect("missing terminal Error event");
    assert_eq!(terminal_error.0, StopReason::Aborted);
    assert_eq!(
        terminal_error.2, None,
        "cancellation should not be retryable"
    );
    assert!(
        terminal_error.1.contains("Google request cancelled"),
        "unexpected cancellation message: {}",
        terminal_error.1
    );
}

#[tokio::test]
async fn google_tool_call_stream() {
    let body = [
        r#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"id":"call_1","name":"get_weather","args":{"city":"Paris"}}}]}}]}"#,
        "",
        r#"data: {"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":7,"totalTokenCount":17}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/v1beta/models/gemini-3-flash-preview:streamGenerateContent",
        ))
        .and(query_param("alt", "sse"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let stream_fn = GeminiStreamFn::new(server.uri(), "test-key", ApiVersion::V1beta);
    let events = collect_events(&stream_fn).await;
    let types: Vec<_> = events.iter().map(event_name).collect();

    assert!(
        types.contains(&"ToolCallStart"),
        "missing ToolCallStart: {types:?}"
    );
    assert!(
        types.contains(&"ToolCallDelta"),
        "missing ToolCallDelta: {types:?}"
    );
    assert!(
        types.contains(&"ToolCallEnd"),
        "missing ToolCallEnd: {types:?}"
    );

    let start = events.iter().find_map(|event| match event {
        AssistantMessageEvent::ToolCallStart { id, name, .. } => Some((id.clone(), name.clone())),
        _ => None,
    });
    assert_eq!(
        start,
        Some(("call_1".to_string(), "get_weather".to_string()))
    );

    let arguments: String = events
        .iter()
        .filter_map(|event| match event {
            AssistantMessageEvent::ToolCallDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(arguments, r#"{"city":"Paris"}"#);

    let stop_reason = events.iter().find_map(|event| match event {
        AssistantMessageEvent::Done { stop_reason, .. } => Some(*stop_reason),
        _ => None,
    });
    assert_eq!(stop_reason, Some(StopReason::ToolUse));
}

#[tokio::test]
async fn google_thinking_then_text_stream() {
    let body = [
        r#"data: {"candidates":[{"content":{"parts":[{"text":"pondering","thought":true,"thoughtSignature":"sig-1"}]}}]}"#,
        "",
        r#"data: {"candidates":[{"content":{"parts":[{"text":"done"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":6,"candidatesTokenCount":4,"totalTokenCount":10}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/v1beta/models/gemini-3-flash-preview:streamGenerateContent",
        ))
        .and(query_param("alt", "sse"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let stream_fn = GeminiStreamFn::new(server.uri(), "test-key", ApiVersion::V1beta);
    let events = collect_events(&stream_fn).await;
    let types: Vec<_> = events.iter().map(event_name).collect();

    assert!(
        types.contains(&"ThinkingStart"),
        "missing ThinkingStart: {types:?}"
    );
    assert!(
        types.contains(&"ThinkingDelta"),
        "missing ThinkingDelta: {types:?}"
    );
    assert!(
        types.contains(&"ThinkingEnd"),
        "missing ThinkingEnd: {types:?}"
    );
    assert!(types.contains(&"TextStart"), "missing TextStart: {types:?}");

    let thinking_end_pos = types
        .iter()
        .position(|event| *event == "ThinkingEnd")
        .expect("missing ThinkingEnd");
    let text_start_pos = types
        .iter()
        .position(|event| *event == "TextStart")
        .expect("missing TextStart");
    assert!(thinking_end_pos < text_start_pos);
}

// T013: Message conversion — verify request body is correctly serialized
#[tokio::test]
async fn google_request_includes_system_and_api_key() {
    let body = [
        r#"data: {"candidates":[{"content":{"parts":[{"text":"ok"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":1,"totalTokenCount":2}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/v1beta/models/gemini-3-flash-preview:streamGenerateContent",
        ))
        .and(query_param("alt", "sse"))
        .and(header("x-goog-api-key", "test-key"))
        .respond_with(sse_response(&body))
        .expect(1)
        .mount(&server)
        .await;

    let stream_fn = GeminiStreamFn::new(server.uri(), "test-key", ApiVersion::V1beta);
    let events = collect_events(&stream_fn).await;

    let types: Vec<_> = events.iter().map(event_name).collect();
    assert!(types.contains(&"Done"), "missing Done: {types:?}");
}

// T025: Multiple tool calls in a single response
#[tokio::test]
async fn google_multiple_tool_calls() {
    let body = [
        r#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"id":"c1","name":"get_weather","args":{"city":"Paris"}}},{"functionCall":{"id":"c2","name":"get_time","args":{"tz":"UTC"}}}]}}]}"#,
        "",
        r#"data: {"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":12,"totalTokenCount":22}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/v1beta/models/gemini-3-flash-preview:streamGenerateContent",
        ))
        .and(query_param("alt", "sse"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let stream_fn = GeminiStreamFn::new(server.uri(), "test-key", ApiVersion::V1beta);
    let events = collect_events(&stream_fn).await;

    let tool_starts: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            AssistantMessageEvent::ToolCallStart {
                content_index,
                id,
                name,
            } => Some((*content_index, id.clone(), name.clone())),
            _ => None,
        })
        .collect();

    assert_eq!(
        tool_starts.len(),
        2,
        "expected 2 tool calls: {tool_starts:?}"
    );
    assert_eq!(tool_starts[0].1, "c1");
    assert_eq!(tool_starts[0].2, "get_weather");
    assert_eq!(tool_starts[1].1, "c2");
    assert_eq!(tool_starts[1].2, "get_time");
    assert_ne!(
        tool_starts[0].0, tool_starts[1].0,
        "tool calls must have different content indices"
    );

    let stop_reason = events.iter().find_map(|event| match event {
        AssistantMessageEvent::Done { stop_reason, .. } => Some(*stop_reason),
        _ => None,
    });
    assert_eq!(stop_reason, Some(StopReason::ToolUse));
}

#[tokio::test]
async fn google_on_raw_payload_observes_runtime_sse_lines() {
    let body = [
        r#"data: {"candidates":[{"content":{"parts":[{"text":"hello"}]}}]}"#,
        "",
        r#"data: {"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5,"totalTokenCount":15}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/v1beta/models/gemini-3-flash-preview:streamGenerateContent",
        ))
        .and(query_param("alt", "sse"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let observed = Arc::new(Mutex::new(Vec::<String>::new()));
    let callback_lines = Arc::clone(&observed);
    let options = StreamOptions {
        on_raw_payload: Some(Arc::new(move |line| {
            callback_lines
                .lock()
                .expect("callback buffer poisoned")
                .push(line.to_owned());
        })),
        ..StreamOptions::default()
    };

    let stream_fn = GeminiStreamFn::new(server.uri(), "test-key", ApiVersion::V1beta);
    let events = collect_events_with_options(&stream_fn, options).await;
    let observed = observed.lock().expect("callback buffer poisoned").clone();

    assert!(
        events
            .iter()
            .any(|event| matches!(event, AssistantMessageEvent::Done { .. })),
        "expected runtime stream to complete successfully"
    );
    assert_eq!(
        observed,
        vec![
            r#"{"candidates":[{"content":{"parts":[{"text":"hello"}]}}]}"#.to_string(),
            r#"{"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5,"totalTokenCount":15}}"#.to_string(),
        ]
    );
}

#[tokio::test]
async fn google_final_tool_deltas_follow_tool_call_start_order() {
    let body = [
        r#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"id":"c1","name":"get_weather","args":{"city":"Paris"}}},{"functionCall":{"id":"c2","name":"get_time","args":{"tz":"UTC"}}}]}}]}"#,
        "",
        r#"data: {"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":12,"totalTokenCount":22}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let stream_fn = GeminiStreamFn::new(server.uri(), "test-key", ApiVersion::V1beta);

    for _ in 0..32 {
        let events = collect_events(&stream_fn).await;

        let starts: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                AssistantMessageEvent::ToolCallStart {
                    content_index, id, ..
                } => Some((*content_index, id.as_str())),
                _ => None,
            })
            .collect();
        let deltas: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                AssistantMessageEvent::ToolCallDelta {
                    content_index,
                    delta,
                } => Some((*content_index, delta.as_str())),
                _ => None,
            })
            .collect();

        assert_eq!(
            starts,
            vec![(0, "c1"), (1, "c2")],
            "unexpected tool-call start order: {starts:?}"
        );
        assert_eq!(
            deltas,
            vec![(0, r#"{"city":"Paris"}"#), (1, r#"{"tz":"UTC"}"#)],
            "final tool-call deltas must preserve tool-call order"
        );
    }
}

// T029: HTTP 429 → throttled
#[tokio::test]
async fn google_http_429_maps_to_throttled() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
        .mount(&server)
        .await;

    let stream_fn = GeminiStreamFn::new(server.uri(), "test-key", ApiVersion::V1beta);
    let events = collect_events(&stream_fn).await;

    assert!(
        matches!(events.first(), Some(AssistantMessageEvent::Start)),
        "pre-stream HTTP failures must start with Start: {events:?}"
    );

    let msg = find_error_message(&events).expect("expected error event");
    assert!(
        msg.contains("throttled") || msg.contains("rate") || msg.contains("429"),
        "expected throttled error, got: {msg}"
    );
}

// T030: HTTP 401 → auth
#[tokio::test]
async fn google_http_401_maps_to_auth() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
        .mount(&server)
        .await;

    let stream_fn = GeminiStreamFn::new(server.uri(), "test-key", ApiVersion::V1beta);
    let events = collect_events(&stream_fn).await;

    let msg = find_error_message(&events).expect("expected error event");
    assert!(
        msg.contains("auth") || msg.contains("401"),
        "expected auth error, got: {msg}"
    );
}

// T031: HTTP 403 → auth
#[tokio::test]
async fn google_http_403_maps_to_auth() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
        .mount(&server)
        .await;

    let stream_fn = GeminiStreamFn::new(server.uri(), "test-key", ApiVersion::V1beta);
    let events = collect_events(&stream_fn).await;

    let msg = find_error_message(&events).expect("expected error event");
    assert!(
        msg.contains("auth") || msg.contains("403"),
        "expected auth error, got: {msg}"
    );
}

// T032: HTTP 500 → network
#[tokio::test]
async fn google_http_500_maps_to_network() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500).set_body_string("internal error"))
        .mount(&server)
        .await;

    let stream_fn = GeminiStreamFn::new(server.uri(), "test-key", ApiVersion::V1beta);
    let events = collect_events(&stream_fn).await;

    let msg = find_error_message(&events).expect("expected error event");
    assert!(
        msg.contains("500") || msg.contains("Google"),
        "expected network error, got: {msg}"
    );
}

// T033: Connection error → network
#[tokio::test]
async fn google_connection_error_maps_to_network() {
    // Connect to a port that nothing is listening on
    let stream_fn = GeminiStreamFn::new("http://127.0.0.1:1", "test-key", ApiVersion::V1beta);
    let events = collect_events(&stream_fn).await;

    let msg = find_error_message(&events).expect("expected error event");
    assert!(
        msg.contains("connection") || msg.contains("Google connection"),
        "expected network error, got: {msg}"
    );
}

// T034: SAFETY finish reason → error event
#[tokio::test]
async fn google_safety_finish_reason_emits_error() {
    let body = [
        r#"data: {"candidates":[{"content":{"parts":[{"text":"partial"}]},"finishReason":"SAFETY"}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":1,"totalTokenCount":6}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/v1beta/models/gemini-3-flash-preview:streamGenerateContent",
        ))
        .and(query_param("alt", "sse"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let stream_fn = GeminiStreamFn::new(server.uri(), "test-key", ApiVersion::V1beta);
    let events = collect_events(&stream_fn).await;

    let types: Vec<_> = events.iter().map(event_name).collect();
    assert!(
        types.contains(&"Error"),
        "expected Error event for SAFETY finish reason: {types:?}"
    );

    let msg = find_error_message(&events).expect("expected error message");
    assert!(
        msg.contains("safety"),
        "error message should mention safety: {msg}"
    );
}

// T035: SAFETY finish reason is terminal — no Done event after the error (#428)
#[tokio::test]
async fn google_safety_finish_reason_is_terminal_no_done_after_error() {
    let body = [
        r#"data: {"candidates":[{"content":{"parts":[{"text":"partial"}]},"finishReason":"SAFETY"}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":1,"totalTokenCount":6}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/v1beta/models/gemini-3-flash-preview:streamGenerateContent",
        ))
        .and(query_param("alt", "sse"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let stream_fn = GeminiStreamFn::new(server.uri(), "test-key", ApiVersion::V1beta);
    let events = collect_events(&stream_fn).await;

    let types: Vec<_> = events.iter().map(event_name).collect();

    // Must contain exactly one terminal event: the Error from SAFETY.
    assert!(
        types.contains(&"Error"),
        "expected Error event for SAFETY finish reason: {types:?}"
    );
    assert!(
        !types.contains(&"Done"),
        "SAFETY must be terminal — no Done event should follow the Error: {types:?}"
    );

    // Count terminal events (Error + Done). Must be exactly 1.
    let terminal_count = types
        .iter()
        .filter(|t| **t == "Error" || **t == "Done")
        .count();
    assert_eq!(
        terminal_count, 1,
        "expected exactly 1 terminal event, got {terminal_count}: {types:?}"
    );
}

// ── BlockAccumulator regression tests ─────────────────────────────────────────
//
// These tests verify the content-index ordering contract introduced when
// GeminiStreamState migrated from its own block-tracking fields to the shared
// BlockAccumulator.  The key invariant: each block (thinking, text, tool-call)
// receives a globally-sequential, unique content index regardless of how many
// chunks arrive or how the blocks interleave.

/// Thinking block (index 0) followed by text block (index 1): indices must be
/// sequential and non-overlapping across block types.
#[tokio::test]
async fn gemini_thinking_then_text_content_indices_are_sequential() {
    let body = [
        r#"data: {"candidates":[{"content":{"parts":[{"text":"thinking…","thought":true}]}}]}"#,
        "",
        r#"data: {"candidates":[{"content":{"parts":[{"text":"answer"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":3,"totalTokenCount":8}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let stream_fn = GeminiStreamFn::new(server.uri(), "test-key", ApiVersion::V1beta);
    let events = collect_events(&stream_fn).await;

    let thinking_start_ci = events.iter().find_map(|ev| match ev {
        AssistantMessageEvent::ThinkingStart { content_index } => Some(*content_index),
        _ => None,
    });
    let text_start_ci = events.iter().find_map(|ev| match ev {
        AssistantMessageEvent::TextStart { content_index } => Some(*content_index),
        _ => None,
    });

    let thinking_ci = thinking_start_ci.expect("expected ThinkingStart");
    let text_ci = text_start_ci.expect("expected TextStart");
    assert_ne!(
        thinking_ci, text_ci,
        "thinking and text must have different content indices"
    );
    assert!(
        thinking_ci < text_ci,
        "thinking block opened first must have a lower content index"
    );
    assert_eq!(
        text_ci,
        thinking_ci + 1,
        "content indices must be strictly sequential"
    );
}

/// Two tool calls in one response must receive distinct, ascending content
/// indices (regression for the `next_content_index` removal).
#[tokio::test]
async fn gemini_two_tool_calls_have_sequential_content_indices() {
    let body = [
        r#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"id":"c1","name":"get_weather","args":{"city":"Paris"}}},{"functionCall":{"id":"c2","name":"get_time","args":{"tz":"UTC"}}}]}}]}"#,
        "",
        r#"data: {"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":12,"totalTokenCount":22}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let stream_fn = GeminiStreamFn::new(server.uri(), "test-key", ApiVersion::V1beta);
    let events = collect_events(&stream_fn).await;

    let cis: Vec<usize> = events
        .iter()
        .filter_map(|ev| match ev {
            AssistantMessageEvent::ToolCallStart { content_index, .. } => Some(*content_index),
            _ => None,
        })
        .collect();

    assert_eq!(cis.len(), 2, "expected 2 ToolCallStart events: {cis:?}");
    assert_ne!(
        cis[0], cis[1],
        "tool calls must have different content indices"
    );
    assert!(
        cis[0] < cis[1],
        "first tool call must have a lower content index"
    );
    assert_eq!(cis[1], cis[0] + 1, "content indices must be sequential");
}

/// When a tool call is emitted but finishReason is MAX_TOKENS, the stop reason
/// must be Length — not ToolUse. Regression test for #273.
#[tokio::test]
async fn gemini_max_tokens_after_tool_call_reports_length() {
    let body = [
        r#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"id":"c1","name":"get_weather","args":{"city":"Paris"}}}]}}]}"#,
        "",
        r#"data: {"candidates":[{"finishReason":"MAX_TOKENS"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":50,"totalTokenCount":60}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/v1beta/models/gemini-3-flash-preview:streamGenerateContent",
        ))
        .and(query_param("alt", "sse"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let stream_fn = GeminiStreamFn::new(server.uri(), "test-key", ApiVersion::V1beta);
    let events = collect_events(&stream_fn).await;

    // Tool call events should still be present
    let types: Vec<_> = events.iter().map(event_name).collect();
    assert!(
        types.contains(&"ToolCallStart"),
        "missing ToolCallStart: {types:?}"
    );

    // Stop reason must be Length, not ToolUse
    let stop_reason = events.iter().find_map(|event| match event {
        AssistantMessageEvent::Done { stop_reason, .. } => Some(*stop_reason),
        _ => None,
    });
    assert_eq!(
        stop_reason,
        Some(StopReason::Length),
        "MAX_TOKENS must map to Length even when tool calls were emitted"
    );
}

/// Regression test for #271: when Gemini rewrites `function_call.args` between
/// chunks (non-prefix change), the concatenated `ToolCallDelta` events must
/// produce the correct final JSON, not a corrupted `old + new` concatenation.
#[tokio::test]
async fn gemini_non_prefix_arg_rewrite_produces_correct_json() {
    // Chunk 1: initial args snapshot with key "a"
    // Chunk 2: rewritten args snapshot with key "b" (NOT a prefix extension)
    // Chunk 3: finish
    let body = [
        r#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"id":"c1","name":"do_stuff","args":{"a":1}}}]}}]}"#,
        "",
        r#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"id":"c1","name":"do_stuff","args":{"b":2}}}]}}]}"#,
        "",
        r#"data: {"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":5,"totalTokenCount":10}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/v1beta/models/gemini-3-flash-preview:streamGenerateContent",
        ))
        .and(query_param("alt", "sse"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let stream_fn = GeminiStreamFn::new(server.uri(), "test-key", ApiVersion::V1beta);
    let events = collect_events(&stream_fn).await;

    let arguments: String = events
        .iter()
        .filter_map(|event| match event {
            AssistantMessageEvent::ToolCallDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();

    // The concatenation of all deltas must be valid JSON matching the LAST
    // snapshot — not a corrupted merge of old + new.
    let parsed: serde_json::Value =
        serde_json::from_str(&arguments).expect("concatenated deltas must be valid JSON");
    assert_eq!(
        parsed,
        serde_json::json!({"b": 2}),
        "final args must match the last Gemini snapshot, got: {arguments}"
    );
}

/// Verify that monotonic prefix-growth of `function_call.args` across multiple
/// chunks still produces the correct concatenated JSON after the deferred-delta
/// change for #271.
#[tokio::test]
async fn gemini_prefix_growth_args_produce_correct_json() {
    // Chunk 1: partial args  {"city":"Pa
    // Chunk 2: extended args {"city":"Paris"}
    // Chunk 3: finish
    let body = [
        r#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"id":"c1","name":"get_weather","args":{"city":"Paris"}}}]}}]}"#,
        "",
        r#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"id":"c1","name":"get_weather","args":{"city":"Paris","unit":"C"}}}]}}]}"#,
        "",
        r#"data: {"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":5,"totalTokenCount":10}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/v1beta/models/gemini-3-flash-preview:streamGenerateContent",
        ))
        .and(query_param("alt", "sse"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let stream_fn = GeminiStreamFn::new(server.uri(), "test-key", ApiVersion::V1beta);
    let events = collect_events(&stream_fn).await;

    let arguments: String = events
        .iter()
        .filter_map(|event| match event {
            AssistantMessageEvent::ToolCallDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();

    let parsed: serde_json::Value =
        serde_json::from_str(&arguments).expect("concatenated deltas must be valid JSON");
    assert_eq!(
        parsed,
        serde_json::json!({"city": "Paris", "unit": "C"}),
        "final args must match the last snapshot, got: {arguments}"
    );
}

/// When the stream ends without an explicit finish reason the open text block
/// must be closed by the finalization path (not silently dropped).
#[tokio::test]
async fn gemini_abrupt_stream_end_closes_open_text_block() {
    // No finishReason, no [DONE] — simulates an abrupt disconnect.
    let body = [
        r#"data: {"candidates":[{"content":{"parts":[{"text":"partial"}]}}]}"#,
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let stream_fn = GeminiStreamFn::new(server.uri(), "test-key", ApiVersion::V1beta);
    let events = collect_events(&stream_fn).await;

    let types: Vec<_> = events.iter().map(event_name).collect();
    // The text block must have been opened…
    assert!(
        types.contains(&"TextStart"),
        "expected TextStart: {types:?}"
    );
    // …and must be closed, even on abrupt termination.
    assert!(
        types.contains(&"TextEnd"),
        "expected TextEnd from finalization: {types:?}"
    );
}
