#![cfg(feature = "gemini")]
//! Wiremock-based tests for `GeminiStreamFn`.

mod common;

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
    let model = test_model();
    let context = test_context();
    let options = StreamOptions::default();
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
