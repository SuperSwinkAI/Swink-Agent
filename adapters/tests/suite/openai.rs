//! Wiremock-based tests for `OpenAiStreamFn`.
//!
//! The first part exercises the Chat Completions backend
//! (`OpenAiStreamFn::new_chat_completions`, the OpenAI-compatible path) with
//! Chat Completions SSE fixtures. The "Responses path" section at the end
//! exercises `OpenAiStreamFn::new` with Responses events — the same
//! `AssistantMessageEvent` contract on the other wire.

use std::sync::{Arc, Mutex};

use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use swink_agent::{
    AssistantMessageEvent, ModelSpec, StopReason, StreamErrorKind, StreamFn, StreamOptions,
};
use swink_agent_adapters::{HeaderName, HeaderValue, OpenAiStreamFn};

use crate::common::{
    event_name, find_error_kind, find_error_message, notify_on_request, sse_response, test_context,
};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn test_model() -> ModelSpec {
    ModelSpec::new("openai", "gpt-4")
}

async fn collect_events(stream_fn: &OpenAiStreamFn) -> Vec<AssistantMessageEvent> {
    collect_events_with_options(stream_fn, StreamOptions::default()).await
}

async fn collect_events_with_options(
    stream_fn: &OpenAiStreamFn,
    options: StreamOptions,
) -> Vec<AssistantMessageEvent> {
    let model = test_model();
    let context = test_context();
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

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
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

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
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
async fn openai_tool_call_name_can_arrive_after_arguments() {
    let body = [
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"tc_1","function":{"arguments":"{\"cmd\":"}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"bash","arguments":"\"ls\"}"}}]},"index":0}]}"#,
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

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let tool_starts: Vec<(String, String)> = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::ToolCallStart { id, name, .. } => {
                Some((id.clone(), name.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(tool_starts, vec![("tc_1".to_string(), "bash".to_string())]);

    let tool_deltas: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::ToolCallDelta { delta, .. } => Some(delta.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(tool_deltas, vec![r#"{"cmd":"ls"}"#.to_string()]);

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

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
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

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let usage = events.iter().find_map(|e| match e {
        AssistantMessageEvent::Done { usage, .. } => Some(usage.clone()),
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

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let (stop_reason, usage) = events
        .iter()
        .find_map(|e| match e {
            AssistantMessageEvent::Done {
                stop_reason, usage, ..
            } => Some((*stop_reason, usage.clone())),
            _ => None,
        })
        .expect("missing Done event");

    assert_eq!(stop_reason, StopReason::Stop);
    assert_eq!(
        usage.input, 100,
        "expected input tokens from separate chunk"
    );
    assert_eq!(
        usage.output, 25,
        "expected output tokens from separate chunk"
    );
}

#[tokio::test]
async fn openai_on_raw_payload_observes_runtime_sse_lines() {
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

    let observed = Arc::new(Mutex::new(Vec::<String>::new()));
    let callback_lines = Arc::clone(&observed);
    let options = StreamOptions::default().with_on_raw_payload(Arc::new(move |line| {
        callback_lines
            .lock()
            .expect("callback buffer poisoned")
            .push(line.to_owned());
    }));

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
    let events = collect_events_with_options(&sf, options).await;
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
            r#"{"choices":[{"delta":{"content":"hello"},"index":0}]}"#.to_string(),
            r#"{"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#.to_string(),
        ]
    );
}

#[tokio::test]
async fn openai_usage_preserves_provider_total_and_breakdowns() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"hi"},"index":0}]}"#,
        "",
        r#"data: {"choices":[],"usage":{"prompt_tokens":42,"completion_tokens":17,"total_tokens":80,"prompt_tokens_details":{"cached_tokens":9},"completion_tokens_details":{"reasoning_tokens":7,"accepted_prediction_tokens":3},"provider_batch_tokens":11}}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}]}"#,
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

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let usage = events
        .iter()
        .find_map(|e| match e {
            AssistantMessageEvent::Done { usage, .. } => Some(usage.clone()),
            _ => None,
        })
        .expect("missing Done event");

    assert_eq!(usage.input, 42);
    assert_eq!(usage.output, 17);
    assert_eq!(usage.total, 80, "expected provider-reported total");
    assert_eq!(usage.extra["prompt_tokens_details.cached_tokens"], 9);
    assert_eq!(usage.extra["completion_tokens_details.reasoning_tokens"], 7);
    assert_eq!(
        usage.extra["completion_tokens_details.accepted_prediction_tokens"],
        3
    );
    assert_eq!(usage.extra["provider_batch_tokens"], 11);
}

#[tokio::test]
async fn openai_http_401() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_string("Unauthorized"))
        .mount(&server)
        .await;

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
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

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let err = find_error_message(&events).expect("expected error event");
    assert!(
        err.contains("rate limit"),
        "expected 'rate limit', got: {err}"
    );
}

#[tokio::test]
async fn openai_http_400_context_length_exceeded_sets_context_overflow_kind() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            r#"{"error":{"message":"This model's maximum context length is 128000 tokens. However, your messages resulted in 131000 tokens. Please reduce the length of the messages.","type":"invalid_request_error","param":"messages","code":"context_length_exceeded"}}"#,
        ))
        .mount(&server)
        .await;

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    assert!(
        matches!(events.first(), Some(AssistantMessageEvent::Start)),
        "pre-stream HTTP failures must start with Start: {events:?}"
    );
    assert_eq!(
        find_error_kind(&events),
        Some(Some(StreamErrorKind::ContextWindowExceeded)),
        "expected structured ContextWindowExceeded, got: {events:?}"
    );
    let err = find_error_message(&events).expect("expected error event");
    assert!(
        err.contains("maximum context length"),
        "expected provider message preserved, got: {err}"
    );
}

#[tokio::test]
async fn openai_http_400_generic_bad_request_has_no_kind() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            r#"{"error":{"message":"Invalid value for 'temperature'","type":"invalid_request_error","param":"temperature","code":null}}"#,
        ))
        .mount(&server)
        .await;

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    assert_eq!(
        find_error_kind(&events),
        Some(None),
        "generic 400 must stay unclassified, got: {events:?}"
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

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
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

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
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

    let (slow_response, request_seen) =
        notify_on_request(sse_response(&body).set_delay(std::time::Duration::from_secs(30)));

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(slow_response)
        .mount(&server)
        .await;

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
    let model = test_model();
    let context = test_context();
    let options = StreamOptions::default();
    let token = CancellationToken::new();

    let cancel_token = token.clone();
    let events_handle = tokio::spawn(async move {
        sf.stream(&model, &context, &options, token)
            .collect::<Vec<_>>()
            .await
    });

    request_seen.notified().await;
    cancel_token.cancel();
    let events = events_handle.await.expect("stream task should complete");

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
async fn openai_pre_send_cancellation_skips_http_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(""))
        .expect(0)
        .mount(&server)
        .await;

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
    let model = test_model();
    let context = test_context();
    let options = StreamOptions::default();
    let token = CancellationToken::new();
    token.cancel();

    let events: Vec<_> = sf.stream(&model, &context, &options, token).collect().await;

    assert!(matches!(events.first(), Some(AssistantMessageEvent::Start)));
    assert!(events.iter().any(|event| {
        matches!(
            event,
            AssistantMessageEvent::Error {
                stop_reason: StopReason::Aborted,
                error_message,
                ..
            } if error_message.contains("cancelled")
        )
    }));

    let received = server.received_requests().await.expect("request log");
    assert!(
        received.is_empty(),
        "expected no HTTP request, got {received:?}"
    );
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

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
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

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "default-key");
    let model = test_model();
    let context = test_context();
    let options = StreamOptions::default().with_api_key("override-key");
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
    let sf = OpenAiStreamFn::new_chat_completions("https://api.openai.com", "sk-secret-key-12345");
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

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
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

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
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

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
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
async fn openai_content_filter_is_terminal_error() {
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

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    let error_event = events
        .iter()
        .find(|e| matches!(e, AssistantMessageEvent::Error { .. }));
    assert!(
        error_event.is_some(),
        "expected a content-filter terminal error, got: {events:?}"
    );
    assert!(
        matches!(
            error_event,
            Some(AssistantMessageEvent::Error {
                error_kind: Some(swink_agent::StreamErrorKind::ContentFiltered),
                ..
            })
        ),
        "expected ContentFiltered error kind, got: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::Done { .. })),
        "content_filter should stop the stream without a trailing Done: {events:?}"
    );

    // Exactly one terminal event total
    let terminal_count = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. }
            )
        })
        .count();
    assert_eq!(
        terminal_count, 1,
        "exactly one terminal event expected, got {terminal_count}"
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

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
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

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
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
async fn openai_eof_after_finish_reason_is_network_error() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"partial"},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":5,"completion_tokens":3}}"#,
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

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AssistantMessageEvent::Done { .. })),
        "EOF before [DONE] must not synthesize Done: {events:?}"
    );
    let err = find_error_message(&events).expect("expected error event");
    assert!(
        err.contains("stream ended unexpectedly"),
        "expected 'stream ended unexpectedly', got: {err}"
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

    let sf = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
    let events = collect_events(&sf).await;

    // Empty SSE body means stream ends immediately without [DONE].
    // Adapter should emit Start then an error about unexpected stream end.
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

/// `ServingOptions`: `top_p` serializes as a typed body field and `extra`
/// keys merge into the top-level body; keys colliding with typed fields are
/// dropped (typed fields win). `context_length`/`keep_alive` have no OAI
/// equivalent and must not appear.
#[tokio::test]
async fn serving_options_serialize_into_request_body() {
    use wiremock::matchers::body_string_contains;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("\"top_p\":0.9"))
        .and(body_string_contains("\"logit_bias\":{\"50256\":-100}"))
        .respond_with(sse_response(
            &[
                r#"data: {"choices":[{"delta":{"content":"ok"},"index":0}]}"#,
                "",
                r#"data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}]}"#,
                "",
                "data: [DONE]",
                "",
                "",
            ]
            .join("\n"),
        ))
        .expect(1)
        .mount(&server)
        .await;

    let stream_fn = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
    let options = StreamOptions::default().with_serving(
        swink_agent::ServingOptions::default()
            .with_context_length(8192)
            .with_top_p(0.9)
            .with_keep_alive("5m")
            .with_extra(
                [
                    ("logit_bias".to_string(), serde_json::json!({"50256": -100})),
                    // Colliding key: the typed `top_p` above must win.
                    ("top_p".to_string(), serde_json::json!(0.1)),
                ]
                .into_iter()
                .collect(),
            ),
    );
    let events = collect_events_with_options(&stream_fn, options).await;
    assert!(matches!(events[0], AssistantMessageEvent::Start));

    let requests = server.received_requests().await.expect("recording enabled");
    let body = String::from_utf8(requests[0].body.clone()).expect("utf8 body");
    assert!(!body.contains("num_ctx"), "no num_ctx on OAI path: {body}");
    assert!(
        !body.contains("context_length"),
        "no context_length: {body}"
    );
    assert!(!body.contains("keep_alive"), "no keep_alive: {body}");
    assert!(
        !body.contains("0.1"),
        "extra top_p must lose to typed: {body}"
    );
}

/// Drive one request with `serving` and return the raw request body sent.
async fn body_for_serving(serving: swink_agent::ServingOptions) -> String {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(
            &[
                r#"data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}]}"#,
                "",
                "data: [DONE]",
                "",
                "",
            ]
            .join("\n"),
        ))
        .expect(1)
        .mount(&server)
        .await;

    let stream_fn = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
    let options = StreamOptions::default().with_serving(serving);
    let _events = collect_events_with_options(&stream_fn, options).await;

    let requests = server.received_requests().await.expect("recording enabled");
    String::from_utf8(requests[0].body.clone()).expect("utf8 body")
}

/// `ResponseFormat::Json` maps onto the OAI protocol's `response_format` field
/// as `{"type": "json_object"}`.
#[tokio::test]
async fn openai_response_format_json_maps_to_json_object() {
    let body = body_for_serving(
        swink_agent::ServingOptions::default().with_format(swink_agent::ResponseFormat::Json),
    )
    .await;

    let json: serde_json::Value = serde_json::from_str(&body).expect("valid JSON body");
    assert_eq!(
        json["response_format"],
        serde_json::json!({ "type": "json_object" }),
        "body: {body}"
    );
    assert!(
        !body.contains("\"format\":"),
        "must use `response_format`, not Ollama's `format`: {body}"
    );
}

/// `ResponseFormat::Schema` is wrapped in the protocol's `json_schema`
/// envelope, carrying the caller's bare JSON Schema verbatim inside it.
#[tokio::test]
async fn openai_response_format_schema_maps_to_json_schema_envelope() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": { "name": { "type": "string" } },
        "required": ["name"],
    });
    let body = body_for_serving(
        swink_agent::ServingOptions::default()
            .with_format(swink_agent::ResponseFormat::Schema(schema.clone())),
    )
    .await;

    let json: serde_json::Value = serde_json::from_str(&body).expect("valid JSON body");
    assert_eq!(json["response_format"]["type"], "json_schema", "{body}");
    assert_eq!(
        json["response_format"]["json_schema"]["schema"], schema,
        "caller's schema must pass through verbatim: {body}"
    );
    assert_eq!(json["response_format"]["json_schema"]["strict"], true);
}

/// With `format: None` the request body is byte-identical to what the adapter
/// emitted before `ServingOptions::format` existed. The literal below was
/// captured from the pre-change adapter; it must not drift.
#[tokio::test]
async fn openai_format_none_body_is_byte_identical() {
    let body = body_for_serving(swink_agent::ServingOptions::default()).await;
    assert_eq!(
        body,
        r#"{"model":"gpt-4","messages":[{"role":"system","content":"You are a test assistant."}],"stream":true,"stream_options":{"include_usage":true}}"#,
        "`format: None` must leave the request body byte-identical"
    );
}

// ── Static header injection (#1260) ──────────────────────────────────────────

/// Minimal SSE body that terminates the stream cleanly.
fn done_body() -> String {
    [
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}]}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n")
}

#[tokio::test]
async fn extra_headers_are_sent_on_every_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("chatgpt-account-id", "acct-123"))
        .and(header("originator", "swink"))
        // The default bearer auth must survive alongside the extras.
        .and(header("authorization", "Bearer test-key"))
        .respond_with(sse_response(&done_body()))
        .expect(2)
        .mount(&server)
        .await;

    let stream_fn = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key")
        .with_header(
            HeaderName::from_static("chatgpt-account-id"),
            HeaderValue::from_static("acct-123"),
        )
        .with_header(
            HeaderName::from_static("originator"),
            HeaderValue::from_static("swink"),
        );

    // Twice, because "every request" is the claim under test.
    collect_events(&stream_fn).await;
    collect_events(&stream_fn).await;
}

#[tokio::test]
async fn no_extra_headers_keeps_the_default_bearer_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(sse_response(&done_body()))
        .expect(1)
        .mount(&server)
        .await;

    let stream_fn = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
    let events = collect_events(&stream_fn).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::Done { .. })),
        "expected a Done event, got {:?}",
        events.iter().map(event_name).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn authorization_header_can_be_overridden() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Token opaque-value"))
        .respond_with(sse_response(&done_body()))
        .expect(1)
        .mount(&server)
        .await;

    let stream_fn = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key").with_header(
        HeaderName::from_static("authorization"),
        HeaderValue::from_static("Token opaque-value"),
    );

    let events = collect_events(&stream_fn).await;
    // A duplicated `Authorization` would have produced two values and failed
    // the matcher; reaching Done proves the default was replaced, not appended.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::Done { .. })),
        "expected a Done event, got {:?}",
        events.iter().map(event_name).collect::<Vec<_>>()
    );
}

#[test]
fn debug_redacts_header_values() {
    let stream_fn = OpenAiStreamFn::new_chat_completions("https://example.test", "test-key")
        .with_header(
            HeaderName::from_static("chatgpt-account-id"),
            HeaderValue::from_static("acct-secret"),
        );
    let rendered = format!("{stream_fn:?}");
    assert!(
        !rendered.contains("acct-secret"),
        "header values must not appear in Debug: {rendered}"
    );
    assert!(!rendered.contains("test-key"), "api key leaked: {rendered}");
}

// ── Rate-limit headers (#1264) ───────────────────────────────────────────────

/// Minimal SSE body that terminates the stream cleanly.
fn rate_limit_done_body() -> String {
    [
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop","index":0}]}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n")
}

/// Codex-style quota headers on a 200 reach the caller intact, exactly once,
/// before the first event.
#[tokio::test]
async fn on_rate_limit_fires_once_before_first_event() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            sse_response(&rate_limit_done_body())
                .insert_header("x-codex-plan-type", "prolite")
                .insert_header("x-codex-primary-used-percent", "98")
                .insert_header("x-codex-primary-window-minutes", "10080")
                .insert_header("x-codex-primary-reset-after-seconds", "288059"),
        )
        .mount(&server)
        .await;

    // One ordered log shared by the callback and the event loop.
    let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let callback_log = Arc::clone(&log);
    let options = StreamOptions::default().with_on_rate_limit(Arc::new(move |snapshot| {
        callback_log.lock().unwrap().push(format!(
            "rate_limit used={:?} plan={:?} resets={:?}",
            snapshot.used_percent, snapshot.plan, snapshot.resets_in
        ));
    }));

    let stream_fn = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
    let model = test_model();
    let context = test_context();
    let mut stream = stream_fn.stream(&model, &context, &options, CancellationToken::new());
    while let Some(event) = stream.next().await {
        log.lock().unwrap().push(event_name(&event).to_owned());
    }

    let log = log.lock().unwrap();
    assert_eq!(
        log[0], "rate_limit used=Some(98.0) plan=Some(\"prolite\") resets=Some(288059s)",
        "callback must run before the first event: {log:?}"
    );
    assert_eq!(
        log.iter().filter(|l| l.starts_with("rate_limit")).count(),
        1,
        "callback must fire exactly once: {log:?}"
    );
    assert_eq!(log[1], "Start");
}

/// A 429's headers are the most important ones; they must reach the caller
/// even though the turn ends in an error.
#[tokio::test]
async fn on_rate_limit_fires_on_error_responses_too() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("x-ratelimit-remaining-requests", "0")
                .insert_header("retry-after", "17")
                .set_body_string(r#"{"error":{"message":"slow down","type":"rate_limit"}}"#),
        )
        .mount(&server)
        .await;

    let seen: Arc<Mutex<Option<swink_agent::RateLimitSnapshot>>> = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&seen);
    let options = StreamOptions::default()
        .with_on_rate_limit(Arc::new(move |s| *sink.lock().unwrap() = Some(s.clone())));

    let stream_fn = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
    let events = collect_events_with_options(&stream_fn, options).await;
    assert!(
        find_error_message(&events).is_some(),
        "expected the 429 to surface as an error"
    );

    let snapshot = seen.lock().unwrap().clone().expect("callback fired on 429");
    assert_eq!(snapshot.remaining_requests, Some(0));
    assert_eq!(snapshot.resets_in, Some(std::time::Duration::from_secs(17)));
}

/// No callback → nothing observes headers and nothing changes.
#[tokio::test]
async fn rate_limit_headers_without_callback_are_ignored() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            sse_response(&rate_limit_done_body())
                .insert_header("x-codex-primary-used-percent", "98"),
        )
        .mount(&server)
        .await;
    let stream_fn = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key");
    let events = collect_events(&stream_fn).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::Done { .. }))
    );
}

/// `RequestBuilder::header` appends; a static `Content-Type` must replace
/// the one `json()` sets, not sit beside it (a duplicate is a 400 on most
/// gateways). Pins the replace semantics of the static header path.
#[tokio::test]
async fn static_content_type_replaces_instead_of_duplicating() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(&done_body()))
        .expect(1)
        .mount(&server)
        .await;

    let stream_fn = OpenAiStreamFn::new_chat_completions(server.uri(), "test-key").with_header(
        HeaderName::from_static("content-type"),
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    collect_events(&stream_fn).await;

    let requests = server.received_requests().await.unwrap();
    let values: Vec<_> = requests[0].headers.get_all("content-type").iter().collect();
    assert_eq!(values.len(), 1, "duplicate Content-Type: {values:?}");
    assert_eq!(values[0], "application/json; charset=utf-8");
    let auth: Vec<_> = requests[0]
        .headers
        .get_all("authorization")
        .iter()
        .collect();
    assert_eq!(auth.len(), 1, "duplicate Authorization: {auth:?}");
}

// ═══════════════════════════════════════════════════════════════════════════
// Responses path — `OpenAiStreamFn::new` (#1266)
// ═══════════════════════════════════════════════════════════════════════════

use swink_agent::{
    AgentMessage, AgentTool, AgentToolResult, AssistantMessage, ContentBlock, LlmMessage,
    ReasoningEffort, ResponseFormat, ServingOptions, ThinkingLevel, ToolResultMessage, UserMessage,
};

fn rsse(events: &[(&str, serde_json::Value)]) -> String {
    let mut out = String::new();
    for (event_type, data) in events {
        out.push_str(&format!("event: {event_type}\ndata: {data}\n\n"));
    }
    out
}

#[allow(clippy::needless_pass_by_value)] // call sites pass `json!` temporaries
fn r_completed(usage: serde_json::Value) -> (&'static str, serde_json::Value) {
    (
        "response.completed",
        serde_json::json!({"type": "response.completed", "response": {"id": "resp_1", "status": "completed", "usage": usage}}),
    )
}

fn r_text(delta: &str) -> (&'static str, serde_json::Value) {
    (
        "response.output_text.delta",
        serde_json::json!({"type": "response.output_text.delta", "output_index": 0, "delta": delta}),
    )
}

fn r_model() -> ModelSpec {
    ModelSpec::new("openai", "gpt-5.6-luna")
}

async fn r_collect(
    stream_fn: &OpenAiStreamFn,
    context: &swink_agent::AgentContext,
    options: StreamOptions,
) -> Vec<AssistantMessageEvent> {
    let model = r_model();
    stream_fn
        .stream(&model, context, &options, CancellationToken::new())
        .collect::<Vec<_>>()
        .await
}

async fn r_last_body(server: &MockServer) -> serde_json::Value {
    let requests = server.received_requests().await.unwrap();
    serde_json::from_slice(&requests.last().unwrap().body).unwrap()
}

struct EchoTool;
static ECHO_SCHEMA: std::sync::LazyLock<serde_json::Value> = std::sync::LazyLock::new(
    || serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}}),
);
impl AgentTool for EchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }
    fn label(&self) -> &'static str {
        "Echo"
    }
    fn description(&self) -> &'static str {
        "Echo the input"
    }
    fn parameters_schema(&self) -> &serde_json::Value {
        &ECHO_SCHEMA
    }
    fn execute(
        &self,
        _tool_call_id: &str,
        _arguments: serde_json::Value,
        _cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: Arc<std::sync::RwLock<swink_agent::SessionState>>,
        _credential: Option<swink_agent::ResolvedCredential>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AgentToolResult> + Send + '_>> {
        Box::pin(async { unreachable!("never executed in these tests") })
    }
}

#[tokio::test]
async fn responses_text_stream_and_request_shape() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(sse_response(&rsse(&[
            r_text("Hel"),
            r_text("lo"),
            r_completed(
                serde_json::json!({"input_tokens": 10, "output_tokens": 2, "total_tokens": 12}),
            ),
        ])))
        .expect(1)
        .mount(&server)
        .await;

    let sf = OpenAiStreamFn::new(server.uri(), "test-key");
    let events = r_collect(&sf, &test_context(), StreamOptions::default()).await;
    assert_eq!(
        events.iter().map(event_name).collect::<Vec<_>>(),
        [
            "Start",
            "TextStart",
            "TextDelta",
            "TextDelta",
            "TextEnd",
            "Done"
        ],
        "{events:?}"
    );
    let body = r_last_body(&server).await;
    assert_eq!(body["model"], "gpt-5.6-luna");
    assert_eq!(body["store"], false);
    assert!(body.get("messages").is_none());
    assert!(body["input"].is_array());
}

/// The whole tool round trip on the Responses wire: turn 1 yields a tool
/// call; turn 2 replays it as `function_call` + `function_call_output` with
/// the same `call_id` and gets the follow-up text.
#[tokio::test]
#[allow(clippy::too_many_lines)] // two full turns, asserted on the wire
async fn responses_tool_round_trip_including_the_follow_up_turn() {
    let server = MockServer::start().await;
    let turn1 = rsse(&[
        (
            "response.output_item.added",
            serde_json::json!({"type": "response.output_item.added", "output_index": 0, "item": {"type": "function_call", "id": "fc_1", "call_id": "call_echo", "name": "echo", "arguments": ""}}),
        ),
        (
            "response.function_call_arguments.delta",
            serde_json::json!({"type": "response.function_call_arguments.delta", "output_index": 0, "delta": "{\"text\":\"hi\"}"}),
        ),
        (
            "response.output_item.done",
            serde_json::json!({"type": "response.output_item.done", "output_index": 0, "item": {"type": "function_call", "id": "fc_1", "call_id": "call_echo", "name": "echo", "arguments": "{\"text\":\"hi\"}"}}),
        ),
        r_completed(serde_json::json!({"input_tokens": 5, "output_tokens": 3})),
    ]);
    let turn2 = rsse(&[
        r_text("done"),
        r_completed(serde_json::json!({"input_tokens": 9, "output_tokens": 1})),
    ]);
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .and(wiremock::matchers::body_string_contains(
            "function_call_output",
        ))
        .respond_with(sse_response(&turn2))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(sse_response(&turn1))
        .expect(1)
        .mount(&server)
        .await;

    let sf = OpenAiStreamFn::new(server.uri(), "test-key");
    let tools: Vec<Arc<dyn AgentTool>> = vec![Arc::new(EchoTool)];
    let user = || {
        AgentMessage::Llm(LlmMessage::User(UserMessage::new(vec![
            ContentBlock::Text {
                text: "say hi".into(),
            },
        ])))
    };

    // Turn 1: model asks for the tool.
    let ctx1 = swink_agent::AgentContext::new("sys", vec![user()], tools.clone());
    let events = r_collect(&sf, &ctx1, StreamOptions::default()).await;
    assert_eq!(
        events.iter().map(event_name).collect::<Vec<_>>(),
        [
            "Start",
            "ToolCallStart",
            "ToolCallDelta",
            "ToolCallEnd",
            "Done"
        ],
        "{events:?}"
    );
    let (id, name) = match &events[1] {
        AssistantMessageEvent::ToolCallStart { id, name, .. } => (id.clone(), name.clone()),
        other => panic!("{other:?}"),
    };
    assert_eq!((id.as_str(), name.as_str()), ("call_echo", "echo"));
    assert!(matches!(
        events[4],
        AssistantMessageEvent::Done {
            stop_reason: StopReason::ToolUse,
            ..
        }
    ));
    let body1 = r_last_body(&server).await;
    assert_eq!(body1["tools"][0]["name"], "echo");
    assert!(
        body1["tools"][0].get("function").is_none(),
        "flat tool schema"
    );

    // Turn 2: replay the assistant tool call + result, expect follow-up text.
    let assistant = AssistantMessage::new(
        vec![ContentBlock::ToolCall {
            id: id.clone(),
            name,
            arguments: serde_json::json!({"text": "hi"}),
            partial_json: None,
        }],
        "openai",
        "gpt-5.6-luna",
    );
    let result = ToolResultMessage::new(id.clone(), vec![ContentBlock::Text { text: "hi".into() }]);
    let ctx2 = swink_agent::AgentContext::new(
        "sys",
        vec![
            user(),
            AgentMessage::Llm(LlmMessage::Assistant(assistant)),
            AgentMessage::Llm(LlmMessage::ToolResult(result)),
        ],
        tools,
    );
    let events = r_collect(&sf, &ctx2, StreamOptions::default()).await;
    assert_eq!(
        events.iter().map(event_name).collect::<Vec<_>>(),
        ["Start", "TextStart", "TextDelta", "TextEnd", "Done"],
        "{events:?}"
    );
    let body2 = r_last_body(&server).await;
    let input = body2["input"].as_array().unwrap();
    assert_eq!(input[0]["type"], "message");
    assert_eq!(input[1]["type"], "function_call");
    assert_eq!(input[1]["call_id"], "call_echo");
    assert_eq!(input[1]["arguments"], r#"{"text":"hi"}"#);
    assert_eq!(input[2]["type"], "function_call_output");
    assert_eq!(input[2]["call_id"], "call_echo");
    assert_eq!(input[2]["output"], "hi");
}

/// Cost-accounting regression: `input_tokens` / `output_tokens` (and the
/// cached split) must land in the right `Usage` fields, and price at the
/// catalog's `openai` rates for the slug.
#[tokio::test]
async fn responses_usage_maps_and_prices_correctly() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse_response(&rsse(&[
            r_text("x"),
            r_completed(serde_json::json!({
                "input_tokens": 1_000_000, "output_tokens": 1_000_000, "total_tokens": 2_000_000,
                "input_tokens_details": {"cached_tokens": 500_000},
                "output_tokens_details": {"reasoning_tokens": 250_000}
            })),
        ])))
        .mount(&server)
        .await;
    let sf = OpenAiStreamFn::new(server.uri(), "test-key");
    let events = r_collect(&sf, &test_context(), StreamOptions::default()).await;
    let usage = match events.last().unwrap() {
        AssistantMessageEvent::Done { usage, .. } => usage.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(usage.input, 500_000, "fresh input = input_tokens - cached");
    assert_eq!(usage.cache_read, 500_000);
    assert_eq!(usage.output, 1_000_000);
    assert_eq!(usage.total, 2_000_000);
    assert_eq!(
        usage.extra["output_tokens_details.reasoning_tokens"],
        250_000
    );

    // gpt-5.6-luna: $0.20/M input, $0.02/M cached, $1.20/M output.
    let cost = swink_agent::calculate_cost_for_provider("openai", "gpt-5.6-luna", &usage);
    assert!((cost.input - 0.10).abs() < 1e-9, "{cost:?}");
    assert!((cost.cache_read - 0.01).abs() < 1e-9, "{cost:?}");
    assert!((cost.output - 1.20).abs() < 1e-9, "{cost:?}");
    assert!((cost.total - 1.31).abs() < 1e-9, "{cost:?}");
}

#[tokio::test]
async fn responses_format_json_and_schema_map_to_text_format() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse_response(&rsse(&[
            r_text("{}"),
            r_completed(serde_json::json!({"input_tokens": 1, "output_tokens": 1})),
        ])))
        .expect(2)
        .mount(&server)
        .await;
    let sf = OpenAiStreamFn::new(server.uri(), "test-key");

    let json = StreamOptions::default()
        .with_serving(ServingOptions::default().with_format(ResponseFormat::Json));
    r_collect(&sf, &test_context(), json).await;
    assert_eq!(
        r_last_body(&server).await["text"]["format"]["type"],
        "json_object"
    );

    let schema = serde_json::json!({"type": "object", "properties": {"ok": {"type": "boolean"}}});
    let opts = StreamOptions::default().with_serving(
        ServingOptions::default().with_format(ResponseFormat::Schema(schema.clone())),
    );
    r_collect(&sf, &test_context(), opts).await;
    let body = r_last_body(&server).await;
    assert_eq!(body["text"]["format"]["type"], "json_schema");
    assert_eq!(body["text"]["format"]["strict"], true);
    assert_eq!(body["text"]["format"]["schema"], schema);
    assert!(
        body.get("response_format").is_none(),
        "Chat Completions field must not leak"
    );
}

#[tokio::test]
async fn responses_reasoning_effort_serving_option_and_model_level() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse_response(&rsse(&[
            r_text("x"),
            r_completed(serde_json::json!({"input_tokens": 1, "output_tokens": 1})),
        ])))
        .expect(2)
        .mount(&server)
        .await;
    let sf = OpenAiStreamFn::new(server.uri(), "test-key");
    assert!(sf.supported_serving_options().reasoning_effort);

    // Per-request override wins.
    let opts = StreamOptions::default()
        .with_serving(ServingOptions::default().with_reasoning_effort(ReasoningEffort::XHigh));
    r_collect(&sf, &test_context(), opts).await;
    assert_eq!(r_last_body(&server).await["reasoning"]["effort"], "xhigh");

    // Model level is the fallback.
    let model = r_model().with_thinking_level(ThinkingLevel::Low);
    sf.stream(
        &model,
        &test_context(),
        &StreamOptions::default(),
        CancellationToken::new(),
    )
    .collect::<Vec<_>>()
    .await;
    assert_eq!(r_last_body(&server).await["reasoning"]["effort"], "low");
}

#[tokio::test]
async fn responses_http_errors_classify_like_chat_completions() {
    for (status, body, expected) in [
        (
            401,
            r#"{"error":{"message":"bad key","type":"invalid_request_error"}}"#,
            Some(StreamErrorKind::Auth),
        ),
        (
            429,
            r#"{"error":{"message":"slow","type":"rate_limit_exceeded"}}"#,
            Some(StreamErrorKind::Throttled),
        ),
        (
            400,
            r#"{"error":{"message":"too long","code":"context_length_exceeded"}}"#,
            Some(StreamErrorKind::ContextWindowExceeded),
        ),
        (400, r#"{"error":{"message":"nope"}}"#, None),
        (500, "boom", Some(StreamErrorKind::Network)),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status).set_body_string(body))
            .mount(&server)
            .await;
        let sf = OpenAiStreamFn::new(server.uri(), "test-key");
        let events = r_collect(&sf, &test_context(), StreamOptions::default()).await;
        assert_eq!(
            find_error_kind(&events),
            Some(expected),
            "status {status}: {events:?}"
        );
    }
}

#[tokio::test]
async fn responses_static_headers_replace_and_api_key_override() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .and(header("authorization", "Bearer per-request"))
        .and(header("openai-organization", "org-1"))
        .respond_with(sse_response(&rsse(&[r_completed(
            serde_json::json!({"input_tokens": 1, "output_tokens": 0}),
        )])))
        .expect(1)
        .mount(&server)
        .await;
    let sf = OpenAiStreamFn::new(server.uri(), "static-key")
        .with_header(
            HeaderName::from_static("openai-organization"),
            HeaderValue::from_static("org-1"),
        )
        .with_header(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/json"),
        );
    let events = r_collect(
        &sf,
        &test_context(),
        StreamOptions::default().with_api_key("per-request"),
    )
    .await;
    assert!(
        matches!(events.last(), Some(AssistantMessageEvent::Done { .. })),
        "{events:?}"
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests[0].headers.get_all("content-type").iter().count(),
        1,
        "no duplicate Content-Type"
    );
    assert_eq!(
        requests[0].headers.get_all("authorization").iter().count(),
        1
    );
}

#[tokio::test]
async fn responses_on_rate_limit_fires_once_before_first_event() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            sse_response(&rsse(&[
                r_text("x"),
                r_completed(serde_json::json!({"input_tokens": 1, "output_tokens": 1})),
            ]))
            .insert_header("x-ratelimit-remaining-requests", "41")
            .insert_header("x-ratelimit-reset-requests", "2m0s"),
        )
        .mount(&server)
        .await;
    let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&log);
    let options = StreamOptions::default().with_on_rate_limit(Arc::new(move |s| {
        sink.lock().unwrap().push(format!(
            "rate_limit {:?} {:?}",
            s.remaining_requests, s.resets_in
        ));
    }));
    let sf = OpenAiStreamFn::new(server.uri(), "test-key");
    let model = r_model();
    let context = test_context();
    let mut stream = sf.stream(&model, &context, &options, CancellationToken::new());
    while let Some(event) = stream.next().await {
        log.lock().unwrap().push(event_name(&event).to_owned());
    }
    let log = log.lock().unwrap();
    assert_eq!(log[0], "rate_limit Some(41) Some(120s)", "{log:?}");
    assert_eq!(
        log.iter().filter(|l| l.starts_with("rate_limit")).count(),
        1
    );
}

#[tokio::test]
async fn responses_pre_send_cancellation_skips_the_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse_response(""))
        .expect(0)
        .mount(&server)
        .await;
    let token = CancellationToken::new();
    token.cancel();
    let sf = OpenAiStreamFn::new(server.uri(), "test-key");
    let model = r_model();
    let events = sf
        .stream(&model, &test_context(), &StreamOptions::default(), token)
        .collect::<Vec<_>>()
        .await;
    assert_eq!(
        events.iter().map(event_name).collect::<Vec<_>>(),
        ["Start", "Error"]
    );
    assert!(matches!(
        &events[1],
        AssistantMessageEvent::Error {
            stop_reason: StopReason::Aborted,
            ..
        }
    ));
}

#[tokio::test]
async fn responses_truncated_stream_is_a_network_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse_response(&rsse(&[r_text("cut")])))
        .mount(&server)
        .await;
    let sf = OpenAiStreamFn::new(server.uri(), "test-key");
    let events = r_collect(&sf, &test_context(), StreamOptions::default()).await;
    assert_eq!(
        find_error_kind(&events),
        Some(Some(StreamErrorKind::Network)),
        "{events:?}"
    );
    assert_eq!(
        events.iter().map(event_name).collect::<Vec<_>>(),
        ["Start", "TextStart", "TextDelta", "TextEnd", "Error"]
    );
}
