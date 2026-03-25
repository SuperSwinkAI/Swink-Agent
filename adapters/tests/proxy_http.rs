#![cfg(feature = "proxy")]
//! Integration tests for the HTTP/SSE proxy functionality (`ProxyStreamFn`).
//!
//! These tests exercise the public API end-to-end by standing up a mock HTTP
//! server (wiremock) and verifying that `ProxyStreamFn` correctly sends
//! requests and reconstructs `AssistantMessageEvent` streams from SSE
//! responses.

use futures::StreamExt;
use swink_agent::{
    AgentContext, AssistantMessageEvent, ContentBlock, ModelSpec, StopReason, StreamFn,
    StreamOptions, accumulate_message,
};
use swink_agent_adapters::ProxyStreamFn;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn test_model() -> ModelSpec {
    ModelSpec::new("test", "test-model")
}

fn test_context() -> AgentContext {
    AgentContext {
        system_prompt: "You are a test assistant.".into(),
        messages: Vec::new(),
        tools: Vec::new(),
    }
}

fn sse_response(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("Content-Type", "text/event-stream")
        .set_body_string(body.to_owned())
}

fn text_only_sse_body() -> String {
    [
        r#"data: {"type":"start"}"#,
        "",
        r#"data: {"type":"text_start","content_index":0}"#,
        "",
        r#"data: {"type":"text_delta","content_index":0,"delta":"hello"}"#,
        "",
        r#"data: {"type":"text_end","content_index":0}"#,
        "",
        r#"data: {"type":"done","stop_reason":"stop","usage":{"input":10,"output":20,"cache_read":0,"cache_write":0,"total":30},"cost":{"input":0.01,"output":0.02,"cache_read":0.0,"cache_write":0.0,"total":0.03}}"#,
        "",
        "",
    ]
    .join("\n")
}

fn text_and_tool_call_sse_body() -> String {
    [
        r#"data: {"type":"start"}"#,
        "",
        r#"data: {"type":"text_start","content_index":0}"#,
        "",
        r#"data: {"type":"text_delta","content_index":0,"delta":"Let me read that."}"#,
        "",
        r#"data: {"type":"text_end","content_index":0}"#,
        "",
        r#"data: {"type":"tool_call_start","content_index":1,"id":"tc_1","name":"read_file"}"#,
        "",
        r#"data: {"type":"tool_call_delta","content_index":1,"delta":"{\"path\":"}"#,
        "",
        r#"data: {"type":"tool_call_delta","content_index":1,"delta":"\"foo.rs\"}"}"#,
        "",
        r#"data: {"type":"tool_call_end","content_index":1}"#,
        "",
        r#"data: {"type":"done","stop_reason":"tool_use","usage":{"input":15,"output":25,"cache_read":0,"cache_write":0,"total":40},"cost":{"input":0.015,"output":0.025,"cache_read":0.0,"cache_write":0.0,"total":0.04}}"#,
        "",
        "",
    ]
    .join("\n")
}

async fn collect_events(proxy: &ProxyStreamFn) -> Vec<AssistantMessageEvent> {
    let model = test_model();
    let context = test_context();
    let options = StreamOptions::default();
    let token = CancellationToken::new();
    let stream = proxy.stream(&model, &context, &options, token);
    stream.collect::<Vec<_>>().await
}

// ── 5.1: Successful stream reconstructs correct AssistantMessage ─────────

#[tokio::test]
async fn successful_stream_reconstructs_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/stream"))
        .respond_with(sse_response(&text_only_sse_body()))
        .mount(&server)
        .await;

    let proxy = ProxyStreamFn::new(server.uri(), "test-token");
    let events = collect_events(&proxy).await;
    let msg = accumulate_message(events, "test", "test-model").expect("accumulate should succeed");

    assert_eq!(msg.content.len(), 1);
    assert_eq!(
        msg.content[0],
        ContentBlock::Text {
            text: "hello".into(),
        }
    );
    assert_eq!(msg.stop_reason, StopReason::Stop);
    assert_eq!(msg.usage.input, 10);
    assert_eq!(msg.usage.output, 20);
    assert!((msg.cost.total - 0.03).abs() < f64::EPSILON);
}

// ── 5.2: Text + tool call reconstructs correctly ────────────────────────

#[tokio::test]
async fn text_and_tool_call_reconstruct_correctly() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/stream"))
        .respond_with(sse_response(&text_and_tool_call_sse_body()))
        .mount(&server)
        .await;

    let proxy = ProxyStreamFn::new(server.uri(), "test-token");
    let events = collect_events(&proxy).await;
    let msg = accumulate_message(events, "test", "test-model").expect("accumulate should succeed");

    assert_eq!(msg.content.len(), 2);
    assert_eq!(
        msg.content[0],
        ContentBlock::Text {
            text: "Let me read that.".into(),
        }
    );
    match &msg.content[1] {
        ContentBlock::ToolCall {
            id,
            name,
            arguments,
            ..
        } => {
            assert_eq!(id, "tc_1");
            assert_eq!(name, "read_file");
            assert_eq!(arguments["path"], "foo.rs");
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
    assert_eq!(msg.stop_reason, StopReason::ToolUse);
}

// ── 5.3: Bearer token is sent in Authorization header ───────────────────

#[tokio::test]
async fn bearer_token_sent_in_auth_header() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/stream"))
        .and(header("Authorization", "Bearer test-token"))
        .respond_with(sse_response(&text_only_sse_body()))
        .expect(1)
        .mount(&server)
        .await;

    let proxy = ProxyStreamFn::new(server.uri(), "test-token");
    let events = collect_events(&proxy).await;

    // If the header did not match, the mock would not respond and we would
    // get an error. Verify we got a successful stream instead.
    let has_start = events
        .iter()
        .any(|e| matches!(e, AssistantMessageEvent::Start));
    assert!(has_start, "expected Start event from authenticated request");
}

// ── 5.4: Connection failure produces network error ──────────────────────

#[tokio::test]
async fn connection_failure_produces_network_error() {
    // Port 1 is privileged and will refuse connections.
    let proxy = ProxyStreamFn::new("http://127.0.0.1:1", "token");
    let events = collect_events(&proxy).await;

    assert_eq!(events.len(), 1);
    match &events[0] {
        AssistantMessageEvent::Error {
            error_message,
            stop_reason,
            ..
        } => {
            assert_eq!(*stop_reason, StopReason::Error);
            assert!(
                error_message.contains("network error"),
                "expected 'network error', got: {error_message}"
            );
        }
        other => panic!("expected Error event, got {other:?}"),
    }
}

// ── 5.5: 401 response produces authentication failure ───────────────────

#[tokio::test]
async fn http_401_produces_auth_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/stream"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let proxy = ProxyStreamFn::new(server.uri(), "bad-token");
    let events = collect_events(&proxy).await;

    assert_eq!(events.len(), 1);
    match &events[0] {
        AssistantMessageEvent::Error {
            error_message,
            stop_reason,
            ..
        } => {
            assert_eq!(*stop_reason, StopReason::Error);
            assert!(
                error_message.contains("auth error"),
                "expected 'auth error', got: {error_message}"
            );
        }
        other => panic!("expected Error event, got {other:?}"),
    }
}

// ── 5.6: 429 response produces rate limit error ─────────────────────────

#[tokio::test]
async fn http_429_produces_rate_limit_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/stream"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let proxy = ProxyStreamFn::new(server.uri(), "token");
    let events = collect_events(&proxy).await;

    assert_eq!(events.len(), 1);
    match &events[0] {
        AssistantMessageEvent::Error {
            error_message,
            stop_reason,
            ..
        } => {
            assert_eq!(*stop_reason, StopReason::Error);
            assert!(
                error_message.contains("rate limit"),
                "expected 'rate limit', got: {error_message}"
            );
            assert!(
                error_message.contains("429"),
                "expected '429', got: {error_message}"
            );
        }
        other => panic!("expected Error event, got {other:?}"),
    }
}

// ── 5.7: 504 response produces network error ───────────────────────────

#[tokio::test]
async fn http_504_produces_network_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/stream"))
        .respond_with(ResponseTemplate::new(504))
        .mount(&server)
        .await;

    let proxy = ProxyStreamFn::new(server.uri(), "token");
    let events = collect_events(&proxy).await;

    assert_eq!(events.len(), 1);
    match &events[0] {
        AssistantMessageEvent::Error {
            error_message,
            stop_reason,
            ..
        } => {
            assert_eq!(*stop_reason, StopReason::Error);
            assert!(
                error_message.contains("server error"),
                "expected 'server error', got: {error_message}"
            );
        }
        other => panic!("expected Error event, got {other:?}"),
    }
}

// ── 5.8: Malformed SSE event produces stream error ──────────────────────

#[tokio::test]
async fn malformed_sse_event_produces_stream_error() {
    let body = [
        r#"data: {"type":"start"}"#,
        "",
        "data: {not valid json at all!!!}",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/stream"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let proxy = ProxyStreamFn::new(server.uri(), "token");
    let events = collect_events(&proxy).await;

    let has_malformed_error = events.iter().any(|e| match e {
        AssistantMessageEvent::Error { error_message, .. } => {
            error_message.contains("malformed SSE event JSON")
        }
        _ => false,
    });
    assert!(
        has_malformed_error,
        "expected an error containing 'malformed SSE event JSON', got: {events:?}"
    );
}

// ── 5.9: Mid-stream disconnect produces network error ───────────────────

#[tokio::test]
async fn mid_stream_disconnect_produces_network_error() {
    // Return a partial SSE stream: start event but no terminal event.
    // The stream will end without a Done/Error, which the proxy treats
    // as an unexpected disconnect.
    let body = [
        r#"data: {"type":"start"}"#,
        "",
        r#"data: {"type":"text_start","content_index":0}"#,
        "",
        r#"data: {"type":"text_delta","content_index":0,"delta":"partial"}"#,
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/stream"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let proxy = ProxyStreamFn::new(server.uri(), "token");
    let events = collect_events(&proxy).await;

    let last = events.last().expect("should have at least one event");
    match last {
        AssistantMessageEvent::Error {
            error_message,
            stop_reason,
            ..
        } => {
            assert_eq!(*stop_reason, StopReason::Error);
            assert!(
                error_message.contains("network error"),
                "expected 'network error', got: {error_message}"
            );
        }
        other => panic!("expected terminal Error event, got {other:?}"),
    }
}

// ── 5.10: Cancellation drops connection and yields Aborted ──────────────

#[tokio::test]
async fn cancellation_yields_aborted() {
    // Build a slow SSE response: start event, then a long delay before done.
    let body = text_only_sse_body();
    let slow_response = sse_response(&body).set_delay(std::time::Duration::from_secs(30));

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/stream"))
        .respond_with(slow_response)
        .mount(&server)
        .await;

    let proxy = ProxyStreamFn::new(server.uri(), "token");
    let model = test_model();
    let context = test_context();
    let options = StreamOptions::default();
    let token = CancellationToken::new();

    let cancel_token = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel_token.cancel();
    });

    let stream = proxy.stream(&model, &context, &options, token);
    let events: Vec<_> = stream.collect().await;

    let has_aborted = events.iter().any(|e| match e {
        AssistantMessageEvent::Error { stop_reason, .. } => *stop_reason == StopReason::Aborted,
        _ => false,
    });
    assert!(
        has_aborted,
        "expected an Aborted event after cancellation, got: {events:?}"
    );
}
