//! Wiremock-based tests for `OpenAiStreamFn`.

use agent_harness::{AgentContext, AssistantMessageEvent, ModelSpec, StopReason, StreamFn, StreamOptions};
use agent_harness_adapters::OpenAiStreamFn;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn test_model() -> ModelSpec {
    ModelSpec::new("openai", "gpt-4")
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

async fn collect_events(stream_fn: &OpenAiStreamFn) -> Vec<AssistantMessageEvent> {
    let model = test_model();
    let context = test_context();
    let options = StreamOptions::default();
    let token = CancellationToken::new();
    let stream = stream_fn.stream(&model, &context, &options, token);
    stream.collect::<Vec<_>>().await
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn openai_text_stream() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"hello"},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = OpenAiStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let types: Vec<&str> = events.iter().map(|e| event_name(e)).collect();
    assert!(types.contains(&"Start"), "missing Start: {types:?}");
    assert!(types.contains(&"TextStart"), "missing TextStart: {types:?}");
    assert!(types.contains(&"TextDelta"), "missing TextDelta: {types:?}");
    assert!(types.contains(&"TextEnd"), "missing TextEnd: {types:?}");
    assert!(types.contains(&"Done"), "missing Done: {types:?}");

    // Verify delta content
    let delta_text: String = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(delta_text, "hello");
}

#[tokio::test]
async fn openai_tool_call_stream() {
    let body = [
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"tc_1","function":{"name":"bash","arguments":""}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"cmd\":"}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"ls\"}"}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls","index":0}],"usage":{"prompt_tokens":10,"completion_tokens":20}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = OpenAiStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let types: Vec<&str> = events.iter().map(|e| event_name(e)).collect();
    assert!(types.contains(&"ToolCallStart"), "missing ToolCallStart: {types:?}");
    assert!(types.contains(&"ToolCallDelta"), "missing ToolCallDelta: {types:?}");
    assert!(types.contains(&"ToolCallEnd"), "missing ToolCallEnd: {types:?}");

    // Verify tool call start details
    let start = events.iter().find_map(|e| match e {
        AssistantMessageEvent::ToolCallStart { id, name, .. } => Some((id.clone(), name.clone())),
        _ => None,
    });
    assert_eq!(start, Some(("tc_1".to_string(), "bash".to_string())));

    // Verify stop reason is ToolUse
    let done = events.iter().find_map(|e| match e {
        AssistantMessageEvent::Done { stop_reason, .. } => Some(*stop_reason),
        _ => None,
    });
    assert_eq!(done, Some(StopReason::ToolUse));
}

#[tokio::test]
async fn openai_text_then_tool() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"thinking..."},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"tc_1","function":{"name":"bash","arguments":"{\"cmd\":\"ls\"}"}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls","index":0}],"usage":{"prompt_tokens":10,"completion_tokens":20}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = OpenAiStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let types: Vec<&str> = events.iter().map(|e| event_name(e)).collect();

    // Text must be closed before tool call starts
    let text_end_pos = types.iter().position(|&t| t == "TextEnd").expect("missing TextEnd");
    let tool_start_pos = types.iter().position(|&t| t == "ToolCallStart").expect("missing ToolCallStart");
    assert!(
        text_end_pos < tool_start_pos,
        "TextEnd ({text_end_pos}) should come before ToolCallStart ({tool_start_pos})"
    );
}

#[tokio::test]
async fn openai_usage_captured() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"hi"},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":42,"completion_tokens":17}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = OpenAiStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let usage = events.iter().find_map(|e| match e {
        AssistantMessageEvent::Done { usage, .. } => Some(*usage),
        _ => None,
    });
    let usage = usage.expect("missing Done event");
    assert_eq!(usage.input, 42);
    assert_eq!(usage.output, 17);
}

#[tokio::test]
async fn openai_http_401() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_string("Unauthorized"))
        .mount(&server)
        .await;

    let sf = OpenAiStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let err = find_error_message(&events).expect("expected error event");
    assert!(
        err.contains("auth error"),
        "expected 'auth error', got: {err}"
    );
}

#[tokio::test]
async fn openai_http_429() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(429).set_body_string("Too Many Requests"))
        .mount(&server)
        .await;

    let sf = OpenAiStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let err = find_error_message(&events).expect("expected error event");
    assert!(
        err.contains("rate limit"),
        "expected 'rate limit', got: {err}"
    );
}

#[tokio::test]
async fn openai_http_500() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
        .mount(&server)
        .await;

    let sf = OpenAiStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let err = find_error_message(&events).expect("expected error event");
    assert!(
        err.contains("server error"),
        "expected 'server error', got: {err}"
    );
}

#[tokio::test]
async fn openai_malformed_json() {
    let body = [
        r#"data: {not valid json!!!}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = OpenAiStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let err = find_error_message(&events).expect("expected error event");
    assert!(
        err.contains("parse error") || err.contains("JSON"),
        "expected parse error, got: {err}"
    );
}

#[tokio::test]
async fn openai_cancellation() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"hello"},"index":0}]}"#,
        "",
        // Long delay simulated by wiremock delay
    ]
    .join("\n");

    let slow_response = sse_response(&body).set_delay(std::time::Duration::from_secs(30));

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(slow_response)
        .mount(&server)
        .await;

    let sf = OpenAiStreamFn::new(server.uri(), "test-key");
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
async fn openai_bearer_token_sent() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"hi"},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("Authorization", "Bearer test-key"))
        .respond_with(sse_response(&body))
        .expect(1)
        .mount(&server)
        .await;

    let sf = OpenAiStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let has_start = events.iter().any(|e| matches!(e, AssistantMessageEvent::Start));
    assert!(has_start, "expected Start from authenticated request");
}

#[tokio::test]
async fn openai_debug_redacts_key() {
    let sf = OpenAiStreamFn::new("https://api.openai.com", "sk-secret-key-12345");
    let debug = format!("{sf:?}");
    assert!(
        debug.contains("[REDACTED]"),
        "Debug output should contain [REDACTED], got: {debug}"
    );
    assert!(
        !debug.contains("sk-secret-key-12345"),
        "Debug output should NOT contain the actual key, got: {debug}"
    );
}

#[tokio::test]
async fn openai_done_without_finish_reason() {
    // Stream ends with [DONE] but no finish_reason in any choice
    let body = [
        r#"data: {"choices":[{"delta":{"content":"hello"},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"index":0}],"usage":{"prompt_tokens":5,"completion_tokens":3}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let sf = OpenAiStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let done = events.iter().find_map(|e| match e {
        AssistantMessageEvent::Done { stop_reason, .. } => Some(*stop_reason),
        _ => None,
    });
    assert_eq!(
        done,
        Some(StopReason::Stop),
        "expected Done with Stop when no finish_reason, got: {events:?}"
    );
}

// ── Utility functions ────────────────────────────────────────────────────────

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
