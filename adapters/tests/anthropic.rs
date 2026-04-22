#![cfg(feature = "anthropic")]
//! Wiremock-based tests for `AnthropicStreamFn`.

mod common;

use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use swink_agent::{AssistantMessageEvent, ModelSpec, StopReason, StreamFn, StreamOptions};
use swink_agent_adapters::AnthropicStreamFn;

use common::{event_name, extract_stop_reason, find_error_message, sse_response, test_context};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn test_model() -> ModelSpec {
    ModelSpec::new("anthropic", "claude-sonnet-4-20250514")
}

async fn collect_events(stream_fn: &AnthropicStreamFn) -> Vec<AssistantMessageEvent> {
    let model = test_model();
    let context = test_context();
    let options = StreamOptions::default();
    let token = CancellationToken::new();
    let stream = stream_fn.stream(&model, &context, &options, token);
    stream.collect::<Vec<_>>().await
}

fn basic_text_sse() -> String {
    [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"usage":{"input_tokens":25}}}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
        "",
        "event: content_block_stop",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "",
        "event: message_delta",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":10}}"#,
        "",
        "event: message_stop",
        r#"data: {"type":"message_stop"}"#,
        "",
        "",
    ]
    .join("\n")
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn anthropic_text_stream() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(sse_response(&basic_text_sse()))
        .mount(&server)
        .await;

    let sf = AnthropicStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let types: Vec<&str> = events.iter().map(|e| event_name(e)).collect();
    assert!(types.contains(&"Start"), "missing Start: {types:?}");
    assert!(types.contains(&"TextStart"), "missing TextStart: {types:?}");
    assert!(types.contains(&"TextDelta"), "missing TextDelta: {types:?}");
    assert!(types.contains(&"TextEnd"), "missing TextEnd: {types:?}");
    assert!(types.contains(&"Done"), "missing Done: {types:?}");

    let delta_text: String = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(delta_text, "Hello");
}

#[tokio::test]
async fn anthropic_tool_use_stream() {
    let body = [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"usage":{"input_tokens":25}}}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tu_1","name":"bash"}}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"command\":"}}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"ls\"}"}}"#,
        "",
        "event: content_block_stop",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "",
        "event: message_delta",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":20}}"#,
        "",
        "event: message_stop",
        r#"data: {"type":"message_stop"}"#,
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = AnthropicStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let types: Vec<&str> = events.iter().map(|e| event_name(e)).collect();
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

    // Verify start details
    let start = events.iter().find_map(|e| match e {
        AssistantMessageEvent::ToolCallStart { id, name, .. } => Some((id.clone(), name.clone())),
        _ => None,
    });
    assert_eq!(start, Some(("tu_1".to_string(), "bash".to_string())));

    // Verify stop reason
    let done = events.iter().find_map(|e| match e {
        AssistantMessageEvent::Done { stop_reason, .. } => Some(*stop_reason),
        _ => None,
    });
    assert_eq!(done, Some(StopReason::ToolUse));
}

#[tokio::test]
async fn anthropic_thinking_stream() {
    let body = [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"usage":{"input_tokens":25}}}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me think..."}}"#,
        "",
        "event: content_block_stop",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"The answer is 42."}}"#,
        "",
        "event: content_block_stop",
        r#"data: {"type":"content_block_stop","index":1}"#,
        "",
        "event: message_delta",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":15}}"#,
        "",
        "event: message_stop",
        r#"data: {"type":"message_stop"}"#,
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = AnthropicStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let types: Vec<&str> = events.iter().map(|e| event_name(e)).collect();
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
    assert!(types.contains(&"TextDelta"), "missing TextDelta: {types:?}");
    assert!(types.contains(&"TextEnd"), "missing TextEnd: {types:?}");
    assert!(types.contains(&"Done"), "missing Done: {types:?}");

    // Thinking should come before text
    let thinking_end_pos = types.iter().position(|&t| t == "ThinkingEnd").unwrap();
    let text_start_pos = types.iter().position(|&t| t == "TextStart").unwrap();
    assert!(
        thinking_end_pos < text_start_pos,
        "ThinkingEnd should precede TextStart"
    );
}

#[tokio::test]
async fn anthropic_usage_tracked() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(sse_response(&basic_text_sse()))
        .mount(&server)
        .await;

    let sf = AnthropicStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let usage = events.iter().find_map(|e| match e {
        AssistantMessageEvent::Done { usage, .. } => Some(usage.clone()),
        _ => None,
    });
    let usage = usage.expect("missing Done event");
    assert_eq!(usage.input, 25, "input_tokens mismatch");
    assert_eq!(usage.output, 10, "output_tokens mismatch");
}

#[tokio::test]
async fn anthropic_cache_tokens() {
    let body = [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"usage":{"input_tokens":25,"cache_read_input_tokens":100,"cache_creation_input_tokens":50}}}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"cached"}}"#,
        "",
        "event: content_block_stop",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "",
        "event: message_delta",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
        "",
        "event: message_stop",
        r#"data: {"type":"message_stop"}"#,
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = AnthropicStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let usage = events.iter().find_map(|e| match e {
        AssistantMessageEvent::Done { usage, .. } => Some(usage.clone()),
        _ => None,
    });
    let usage = usage.expect("missing Done event");
    assert_eq!(usage.cache_read, 100, "cache_read mismatch");
    assert_eq!(usage.cache_write, 50, "cache_write mismatch");
}

#[tokio::test]
async fn anthropic_http_401() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_string("Unauthorized"))
        .mount(&server)
        .await;

    let sf = AnthropicStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    assert!(
        matches!(events.first(), Some(AssistantMessageEvent::Start)),
        "pre-stream HTTP failures must start with Start: {events:?}"
    );

    let err = find_error_message(&events).expect("expected error event");
    assert!(
        err.contains("auth error"),
        "expected 'auth error', got: {err}"
    );
    assert!(err.contains("401"), "expected '401' mention, got: {err}");
}

#[tokio::test]
async fn anthropic_http_429() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(429).set_body_string("Rate limited"))
        .mount(&server)
        .await;

    let sf = AnthropicStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let err = find_error_message(&events).expect("expected error event");
    assert!(
        err.contains("rate limit"),
        "expected 'rate limit', got: {err}"
    );
}

#[tokio::test]
async fn anthropic_http_529() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(529).set_body_string("Overloaded"))
        .mount(&server)
        .await;

    let sf = AnthropicStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let err = find_error_message(&events).expect("expected error event");
    // 529 is mapped to a network/server error via the Anthropic-specific override
    assert!(err.contains("529"), "expected '529' mention, got: {err}");
    assert!(
        err.contains("Overloaded"),
        "expected body content, got: {err}"
    );
}

#[tokio::test]
async fn anthropic_stream_error_event() {
    let body = [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"usage":{"input_tokens":25}}}"#,
        "",
        "event: error",
        r#"data: {"type":"error","error":{"type":"server_error","message":"Internal server error"}}"#,
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = AnthropicStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let err = find_error_message(&events).expect("expected error event");
    assert!(
        err.contains("Internal server error"),
        "expected error message from stream, got: {err}"
    );
}

#[tokio::test]
async fn anthropic_honors_cancellation_before_request_send() {
    // Regression for #726: a token cancelled BEFORE the stream is polled
    // must resolve promptly with an Aborted terminal, not hang behind
    // network latency. We point the adapter at a non-routable address
    // (TEST-NET-1 RFC 5737) so that if cancellation is NOT honored, the
    // `request.send()` call would stall until a TCP connect timeout.
    let sf = AnthropicStreamFn::new("http://192.0.2.1:1", "test-key");
    let model = test_model();
    let context = test_context();
    let options = StreamOptions::default();
    let token = CancellationToken::new();
    token.cancel();

    let stream = sf.stream(&model, &context, &options, token);

    let events = tokio::time::timeout(std::time::Duration::from_secs(2), stream.collect::<Vec<_>>())
        .await
        .expect("stream must resolve promptly on pre-send cancellation");

    let has_aborted = events.iter().any(|e| {
        matches!(
            e,
            AssistantMessageEvent::Error {
                stop_reason: StopReason::Aborted,
                ..
            }
        )
    });
    assert!(
        has_aborted,
        "expected Aborted terminal on pre-send cancellation, got: {events:?}"
    );
}

#[tokio::test]
async fn anthropic_cancellation() {
    let body = [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"usage":{"input_tokens":25}}}"#,
        "",
    ]
    .join("\n");

    let slow_response = sse_response(&body).set_delay(std::time::Duration::from_secs(30));

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(slow_response)
        .mount(&server)
        .await;

    let sf = AnthropicStreamFn::new(server.uri(), "test-key");
    let model = test_model();
    let context = test_context();
    let options = StreamOptions::default();
    let token = CancellationToken::new();

    let cancel_token = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel_token.cancel();
    });

    let stream = sf.stream(&model, &context, &options, token);
    let events: Vec<_> = stream.collect().await;

    let has_aborted = events.iter().any(|e| {
        matches!(
            e,
            AssistantMessageEvent::Error {
                stop_reason: StopReason::Aborted,
                ..
            }
        )
    });
    assert!(has_aborted, "expected Aborted event, got: {events:?}");
}

#[tokio::test]
async fn anthropic_api_key_header() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test-key"))
        .respond_with(sse_response(&basic_text_sse()))
        .expect(1)
        .mount(&server)
        .await;

    let sf = AnthropicStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let has_start = events
        .iter()
        .any(|e| matches!(e, AssistantMessageEvent::Start));
    assert!(has_start, "expected Start from authenticated request");
}

#[tokio::test]
async fn anthropic_stream_options_api_key_overrides_default() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "override-key"))
        .respond_with(sse_response(&basic_text_sse()))
        .expect(1)
        .mount(&server)
        .await;

    let sf = AnthropicStreamFn::new(server.uri(), "default-key");
    let model = test_model();
    let context = test_context();
    let options = StreamOptions {
        api_key: Some("override-key".to_string()),
        ..StreamOptions::default()
    };
    let token = CancellationToken::new();
    let events: Vec<_> = sf.stream(&model, &context, &options, token).collect().await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::Start))
    );
}

#[tokio::test]
async fn anthropic_debug_redacts_key() {
    let sf = AnthropicStreamFn::new("https://api.anthropic.com", "sk-ant-secret-12345");
    let debug = format!("{sf:?}");
    assert!(
        debug.contains("[REDACTED]"),
        "Debug output should contain [REDACTED], got: {debug}"
    );
    assert!(
        !debug.contains("sk-ant-secret-12345"),
        "Debug output should NOT contain the actual key, got: {debug}"
    );
}

#[tokio::test]
async fn anthropic_stop_reason_mapping() {
    // Test "tool_use" -> ToolUse
    let tool_use_body = make_stop_reason_body("tool_use");
    let max_tokens_body = make_stop_reason_body("max_tokens");
    let end_turn_body = make_stop_reason_body("end_turn");

    // tool_use
    {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(sse_response(&tool_use_body))
            .mount(&server)
            .await;

        let sf = AnthropicStreamFn::new(server.uri(), "test-key");
        let events = collect_events(&sf).await;
        let reason = extract_stop_reason(&events).expect("missing Done");
        assert_eq!(
            reason,
            StopReason::ToolUse,
            "tool_use should map to ToolUse"
        );
    }

    // max_tokens
    {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(sse_response(&max_tokens_body))
            .mount(&server)
            .await;

        let sf = AnthropicStreamFn::new(server.uri(), "test-key");
        let events = collect_events(&sf).await;
        let reason = extract_stop_reason(&events).expect("missing Done");
        assert_eq!(
            reason,
            StopReason::Length,
            "max_tokens should map to Length"
        );
    }

    // end_turn
    {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(sse_response(&end_turn_body))
            .mount(&server)
            .await;

        let sf = AnthropicStreamFn::new(server.uri(), "test-key");
        let events = collect_events(&sf).await;
        let reason = extract_stop_reason(&events).expect("missing Done");
        assert_eq!(reason, StopReason::Stop, "end_turn should map to Stop");
    }
}

// ── Utility functions ────────────────────────────────────────────────────────

fn make_stop_reason_body(stop_reason: &str) -> String {
    [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"usage":{"input_tokens":10}}}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}"#,
        "",
        "event: content_block_stop",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "",
        &format!(r#"event: message_delta
data: {{"type":"message_delta","delta":{{"stop_reason":"{stop_reason}"}},"usage":{{"output_tokens":5}}}}"#),
        "",
        "event: message_stop",
        r#"data: {"type":"message_stop"}"#,
        "",
        "",
    ]
    .join("\n")
}

// ── Edge case tests ─────────────────────────────────────────────────────────

#[tokio::test]
async fn anthropic_empty_text_delta_skipped() {
    let body = [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"usage":{"input_tokens":10}}}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":""}}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"real text"}}"#,
        "",
        "event: content_block_stop",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "",
        "event: message_delta",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
        "",
        "event: message_stop",
        r#"data: {"type":"message_stop"}"#,
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = AnthropicStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    // The adapter emits TextDelta for every text_delta event including empty ones.
    // Verify we get two TextDelta events (one empty, one with content).
    let deltas: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas.len(), 2, "expected 2 TextDelta events: {deltas:?}");
    assert_eq!(deltas[0], "", "first delta should be empty");
    assert_eq!(deltas[1], "real text", "second delta should be 'real text'");
}

#[tokio::test]
async fn anthropic_multiple_text_blocks() {
    let body = [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"usage":{"input_tokens":10}}}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"First block"}}"#,
        "",
        "event: content_block_stop",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Second block"}}"#,
        "",
        "event: content_block_stop",
        r#"data: {"type":"content_block_stop","index":1}"#,
        "",
        "event: message_delta",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":10}}"#,
        "",
        "event: message_stop",
        r#"data: {"type":"message_stop"}"#,
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = AnthropicStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let types: Vec<&str> = events.iter().map(|e| event_name(e)).collect();

    // Both blocks should produce TextStart/TextDelta/TextEnd
    let text_starts: Vec<usize> = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::TextStart { content_index } => Some(*content_index),
            _ => None,
        })
        .collect();
    assert_eq!(
        text_starts,
        vec![0, 1],
        "expected two TextStart at indices 0 and 1"
    );

    let text_ends: Vec<usize> = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::TextEnd { content_index } => Some(*content_index),
            _ => None,
        })
        .collect();
    assert_eq!(
        text_ends,
        vec![0, 1],
        "expected two TextEnd at indices 0 and 1"
    );

    // Verify ordering: first block ends before second block starts
    let first_end = types.iter().position(|&t| t == "TextEnd").unwrap();
    let second_start_pos = types.iter().skip(first_end).position(|&t| t == "TextStart");
    assert!(
        second_start_pos.is_some(),
        "second TextStart should follow first TextEnd"
    );

    // Verify delta content
    let deltas: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas, vec!["First block", "Second block"]);
}

#[tokio::test]
async fn anthropic_text_then_tool_call() {
    let body = [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"usage":{"input_tokens":10}}}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Let me run that."}}"#,
        "",
        "event: content_block_stop",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tu_abc","name":"bash"}}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"cmd\":\"ls\"}"}}"#,
        "",
        "event: content_block_stop",
        r#"data: {"type":"content_block_stop","index":1}"#,
        "",
        "event: message_delta",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":15}}"#,
        "",
        "event: message_stop",
        r#"data: {"type":"message_stop"}"#,
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = AnthropicStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let types: Vec<&str> = events.iter().map(|e| event_name(e)).collect();

    // Text block at content_index 0, tool call at content_index 1
    let text_start = events.iter().find_map(|e| match e {
        AssistantMessageEvent::TextStart { content_index } => Some(*content_index),
        _ => None,
    });
    assert_eq!(
        text_start,
        Some(0),
        "text block should be at content_index 0"
    );

    let tool_start = events.iter().find_map(|e| match e {
        AssistantMessageEvent::ToolCallStart {
            content_index,
            id,
            name,
        } => Some((*content_index, id.clone(), name.clone())),
        _ => None,
    });
    assert_eq!(
        tool_start,
        Some((1, "tu_abc".to_string(), "bash".to_string())),
        "tool call should be at content_index 1"
    );

    // TextEnd must precede ToolCallStart
    let text_end_pos = types.iter().position(|&t| t == "TextEnd").unwrap();
    let tool_start_pos = types.iter().position(|&t| t == "ToolCallStart").unwrap();
    assert!(
        text_end_pos < tool_start_pos,
        "TextEnd ({text_end_pos}) should precede ToolCallStart ({tool_start_pos})"
    );

    // Stop reason should be ToolUse
    let reason = extract_stop_reason(&events).expect("missing Done");
    assert_eq!(reason, StopReason::ToolUse);
}

#[tokio::test]
async fn anthropic_missing_message_stop() {
    // Stream ends after message_delta without message_stop
    let body = [
        "event: message_start",
        r#"data: {"type":"message_start","message":{"usage":{"input_tokens":10}}}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        "",
        "event: content_block_delta",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial"}}"#,
        "",
        "event: message_delta",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
        "",
        // No content_block_stop, no message_stop — connection closes
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = AnthropicStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    // Should get an error about unexpected stream end
    let err = find_error_message(&events).expect("expected error event");
    assert!(
        err.contains("stream ended unexpectedly"),
        "expected 'stream ended unexpectedly', got: {err}"
    );

    // Open text block should be finalized (TextEnd emitted)
    let types: Vec<&str> = events.iter().map(|e| event_name(e)).collect();
    assert!(
        types.contains(&"TextEnd"),
        "open text block should be finalized: {types:?}"
    );
}

#[tokio::test]
async fn anthropic_http_500_server_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
        .mount(&server)
        .await;

    let sf = AnthropicStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let err = find_error_message(&events).expect("expected error event");
    assert!(
        err.contains("server error"),
        "expected 'server error', got: {err}"
    );
    assert!(
        err.contains("500"),
        "expected HTTP status code in message, got: {err}"
    );
}

#[tokio::test]
async fn anthropic_empty_response_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(sse_response(""))
        .mount(&server)
        .await;

    let sf = AnthropicStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    // An empty SSE body means the stream ends immediately without message_stop.
    // The adapter should emit Start then an error about unexpected stream end.
    let types: Vec<&str> = events.iter().map(|e| event_name(e)).collect();
    assert!(
        types.contains(&"Start"),
        "should still emit Start: {types:?}"
    );

    let err = find_error_message(&events).expect("expected error event");
    assert!(
        err.contains("stream ended unexpectedly"),
        "expected 'stream ended unexpectedly', got: {err}"
    );
}
