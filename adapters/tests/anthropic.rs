//! Wiremock-based tests for `AnthropicStreamFn`.

use agent_harness::{AgentContext, AssistantMessageEvent, ModelSpec, StopReason, StreamFn, StreamOptions};
use agent_harness_adapters::AnthropicStreamFn;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn test_model() -> ModelSpec {
    ModelSpec::new("anthropic", "claude-sonnet-4-20250514")
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
    assert!(types.contains(&"ToolCallStart"), "missing ToolCallStart: {types:?}");
    assert!(types.contains(&"ToolCallDelta"), "missing ToolCallDelta: {types:?}");
    assert!(types.contains(&"ToolCallEnd"), "missing ToolCallEnd: {types:?}");

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
    assert!(types.contains(&"ThinkingStart"), "missing ThinkingStart: {types:?}");
    assert!(types.contains(&"ThinkingDelta"), "missing ThinkingDelta: {types:?}");
    assert!(types.contains(&"ThinkingEnd"), "missing ThinkingEnd: {types:?}");
    assert!(types.contains(&"TextStart"), "missing TextStart: {types:?}");
    assert!(types.contains(&"TextDelta"), "missing TextDelta: {types:?}");
    assert!(types.contains(&"TextEnd"), "missing TextEnd: {types:?}");
    assert!(types.contains(&"Done"), "missing Done: {types:?}");

    // Thinking should come before text
    let thinking_end_pos = types.iter().position(|&t| t == "ThinkingEnd").unwrap();
    let text_start_pos = types.iter().position(|&t| t == "TextStart").unwrap();
    assert!(thinking_end_pos < text_start_pos, "ThinkingEnd should precede TextStart");
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
        AssistantMessageEvent::Done { usage, .. } => Some(*usage),
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
        AssistantMessageEvent::Done { usage, .. } => Some(*usage),
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

    let err = find_error_message(&events).expect("expected error event");
    assert!(err.contains("auth error"), "expected 'auth error', got: {err}");
    assert!(err.contains("x-api-key"), "expected 'x-api-key' mention, got: {err}");
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
    assert!(err.contains("rate limit"), "expected 'rate limit', got: {err}");
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
    assert!(err.contains("overloaded"), "expected 'overloaded', got: {err}");
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

    let has_aborted = events.iter().any(|e| matches!(
        e,
        AssistantMessageEvent::Error { stop_reason: StopReason::Aborted, .. }
    ));
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

    let has_start = events.iter().any(|e| matches!(e, AssistantMessageEvent::Start));
    assert!(has_start, "expected Start from authenticated request");
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
        assert_eq!(reason, StopReason::ToolUse, "tool_use should map to ToolUse");
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
        assert_eq!(reason, StopReason::Length, "max_tokens should map to Length");
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

fn event_name(event: &AssistantMessageEvent) -> &'static str {
    match event {
        AssistantMessageEvent::Start => "Start",
        AssistantMessageEvent::TextStart { .. } => "TextStart",
        AssistantMessageEvent::TextDelta { .. } => "TextDelta",
        AssistantMessageEvent::TextEnd { .. } => "TextEnd",
        AssistantMessageEvent::ThinkingStart { .. } => "ThinkingStart",
        AssistantMessageEvent::ThinkingDelta { .. } => "ThinkingDelta",
        AssistantMessageEvent::ThinkingEnd { .. } => "ThinkingEnd",
        AssistantMessageEvent::ToolCallStart { .. } => "ToolCallStart",
        AssistantMessageEvent::ToolCallDelta { .. } => "ToolCallDelta",
        AssistantMessageEvent::ToolCallEnd { .. } => "ToolCallEnd",
        AssistantMessageEvent::Done { .. } => "Done",
        AssistantMessageEvent::Error { .. } => "Error",
    }
}

fn find_error_message(events: &[AssistantMessageEvent]) -> Option<String> {
    events.iter().find_map(|e| match e {
        AssistantMessageEvent::Error { error_message, .. } => Some(error_message.clone()),
        _ => None,
    })
}

fn extract_stop_reason(events: &[AssistantMessageEvent]) -> Option<StopReason> {
    events.iter().find_map(|e| match e {
        AssistantMessageEvent::Done { stop_reason, .. } => Some(*stop_reason),
        _ => None,
    })
}
