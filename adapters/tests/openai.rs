//! Wiremock-based tests for `OpenAiStreamFn`.

use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use agent_harness::{
    AgentContext, AssistantMessageEvent, ModelSpec, StopReason, StreamFn, StreamOptions,
};
use agent_harness_adapters::OpenAiStreamFn;

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
    let text_end_pos = types
        .iter()
        .position(|&t| t == "TextEnd")
        .expect("missing TextEnd");
    let tool_start_pos = types
        .iter()
        .position(|&t| t == "ToolCallStart")
        .expect("missing ToolCallStart");
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
async fn openai_usage_in_separate_chunk() {
    // OpenAI sends finish_reason in one chunk and usage in a separate chunk
    // before [DONE]. This matches real OpenAI behavior with `include_usage: true`.
    let body = [
        r#"data: {"choices":[{"delta":{"content":"hi"},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}]}"#,
        "",
        r#"data: {"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":25}}"#,
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

    let (stop_reason, usage) = events
        .iter()
        .find_map(|e| match e {
            AssistantMessageEvent::Done {
                stop_reason,
                usage,
                ..
            } => Some((*stop_reason, *usage)),
            _ => None,
        })
        .expect("missing Done event");

    assert_eq!(stop_reason, StopReason::Stop);
    assert_eq!(usage.input, 100, "expected input tokens from separate chunk");
    assert_eq!(
        usage.output, 25,
        "expected output tokens from separate chunk"
    );
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
    let body = [r"data: {not valid json!!!}", "", "data: [DONE]", "", ""].join("\n");

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

    let has_start = events
        .iter()
        .any(|e| matches!(e, AssistantMessageEvent::Start));
    assert!(has_start, "expected Start from authenticated request");
}

#[tokio::test]
async fn openai_stream_options_api_key_overrides_default() {
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
        .and(header("Authorization", "Bearer override-key"))
        .respond_with(sse_response(&body))
        .expect(1)
        .mount(&server)
        .await;

    let sf = OpenAiStreamFn::new(server.uri(), "default-key");
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

const fn event_name(event: &AssistantMessageEvent) -> &'static str {
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

// ── Edge case tests ─────────────────────────────────────────────────────────

#[tokio::test]
async fn openai_empty_content_delta_skipped() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":""},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"content":"real text"},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":5,"completion_tokens":3}}"#,
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

    // The adapter skips empty content deltas (checks `!content.is_empty()`),
    // so only one TextDelta with "real text" should appear.
    let deltas: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas, vec!["real text"], "empty delta should be skipped");
}

#[tokio::test]
async fn openai_multiple_tool_calls() {
    let body = [
        // First tool call starts
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"tc_a","function":{"name":"bash","arguments":""}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"cmd\":\"ls\"}"}}]},"index":0}]}"#,
        "",
        // Second tool call starts
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"tc_b","function":{"name":"read_file","arguments":""}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"function":{"arguments":"{\"path\":\"foo.txt\"}"}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls","index":0}],"usage":{"prompt_tokens":10,"completion_tokens":30}}"#,
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

    // Collect all ToolCallStart events
    let tool_starts: Vec<(usize, String, String)> = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::ToolCallStart {
                content_index,
                id,
                name,
            } => Some((*content_index, id.clone(), name.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(tool_starts.len(), 2, "expected 2 ToolCallStart events");
    assert_eq!(tool_starts[0].1, "tc_a");
    assert_eq!(tool_starts[0].2, "bash");
    assert_eq!(tool_starts[1].1, "tc_b");
    assert_eq!(tool_starts[1].2, "read_file");

    // Content indices should be sequential
    assert_eq!(tool_starts[0].0, 0, "first tool at content_index 0");
    assert_eq!(tool_starts[1].0, 1, "second tool at content_index 1");

    // Both tool calls should be ended
    let tool_end_count = events
        .iter()
        .filter(|e| matches!(e, AssistantMessageEvent::ToolCallEnd { .. }))
        .count();
    assert_eq!(tool_end_count, 2, "expected 2 ToolCallEnd events");

    // Stop reason should be ToolUse
    let done = events.iter().find_map(|e| match e {
        AssistantMessageEvent::Done { stop_reason, .. } => Some(*stop_reason),
        _ => None,
    });
    assert_eq!(done, Some(StopReason::ToolUse));
}

#[tokio::test]
async fn openai_content_filter_stop_reason() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"filtered"},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"content_filter","index":0}],"usage":{"prompt_tokens":5,"completion_tokens":2}}"#,
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

    // content_filter maps to the catch-all StopReason::Stop
    let done = events.iter().find_map(|e| match e {
        AssistantMessageEvent::Done { stop_reason, .. } => Some(*stop_reason),
        _ => None,
    });
    assert_eq!(
        done,
        Some(StopReason::Stop),
        "content_filter should map to Stop"
    );
}

#[tokio::test]
async fn openai_empty_choices_array() {
    let body = [
        r#"data: {"choices":[]}"#,
        "",
        r#"data: {"choices":[{"delta":{"content":"hello"},"index":0}]}"#,
        "",
        r#"data: {"choices":[]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":5,"completion_tokens":2}}"#,
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

    // Empty choices are simply skipped; the stream should complete normally
    let types: Vec<&str> = events.iter().map(|e| event_name(e)).collect();
    assert!(types.contains(&"Start"), "missing Start: {types:?}");
    assert!(types.contains(&"TextDelta"), "missing TextDelta: {types:?}");
    assert!(types.contains(&"Done"), "missing Done: {types:?}");

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
async fn openai_missing_done_sentinel() {
    // Stream ends (connection closes) without sending [DONE]
    let body = [
        r#"data: {"choices":[{"delta":{"content":"partial"},"index":0}]}"#,
        "",
        // No finish_reason, no [DONE] — connection closes
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

    // Should get an error about unexpected stream end
    let err = find_error_message(&events).expect("expected error event");
    assert!(
        err.contains("stream ended unexpectedly"),
        "expected 'stream ended unexpectedly', got: {err}"
    );

    // Open text block should be finalized
    let types: Vec<&str> = events.iter().map(|e| event_name(e)).collect();
    assert!(
        types.contains(&"TextEnd"),
        "open text block should be finalized: {types:?}"
    );
}

#[tokio::test]
async fn openai_empty_response_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(""))
        .mount(&server)
        .await;

    let sf = OpenAiStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    // Empty SSE body means stream ends immediately without [DONE].
    // Adapter should emit Start then an error about unexpected stream end.
    let types: Vec<&str> = events.iter().map(|e| event_name(e)).collect();
    assert!(types.contains(&"Start"), "should still emit Start: {types:?}");

    let err = find_error_message(&events).expect("expected error event");
    assert!(
        err.contains("stream ended unexpectedly"),
        "expected 'stream ended unexpectedly', got: {err}"
    );
}
