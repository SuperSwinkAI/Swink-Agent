#![cfg(feature = "xai")]
//! Wiremock-based tests for `XAiStreamFn`.

use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use swink_agent::{AgentContext, AssistantMessageEvent, ModelSpec, StreamFn, StreamOptions};
use swink_agent_adapters::XAiStreamFn;

mod common;

use common::{find_error_kind, find_error_message};

fn test_model() -> ModelSpec {
    ModelSpec::new("xai", "grok-4-1-fast-non-reasoning")
}

async fn collect_events(stream_fn: &XAiStreamFn) -> Vec<AssistantMessageEvent> {
    stream_fn
        .stream(
            &test_model(),
            &AgentContext::new(String::new(), Vec::new(), Vec::new()),
            &StreamOptions::default(),
            CancellationToken::new(),
        )
        .collect::<Vec<AssistantMessageEvent>>()
        .await
}

#[tokio::test]
async fn xai_wrapper_streams_chat_completions() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"ok"}}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let stream_fn = XAiStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&stream_fn).await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::TextDelta { delta, .. } if delta == "ok"))
    );
}

#[tokio::test]
async fn xai_http_errors_use_xai_provider_label() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_string("Unauthorized"))
        .mount(&server)
        .await;

    let stream_fn = XAiStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&stream_fn).await;
    let error = find_error_message(&events).expect("expected error event");

    assert!(
        error.contains("xAI auth error"),
        "expected xAI provider label, got: {error}"
    );
    assert!(
        !error.contains("OpenAI"),
        "xAI errors should not mention OpenAI: {error}"
    );
}

#[tokio::test]
async fn xai_http_400_prompt_length_exceeded_sets_context_overflow_kind() {
    // xAI reports errors as {"code": "...", "error": "message"} with the
    // documented "maximum prompt length" wording for context overflow.
    let error_body = r#"{"code":"Client specified an invalid argument","error":"This model's maximum prompt length is 131072 but the request contains 200000 tokens."}"#;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_string(error_body))
        .mount(&server)
        .await;

    let stream_fn = XAiStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&stream_fn).await;

    assert_eq!(
        find_error_kind(&events),
        Some(Some(swink_agent::StreamErrorKind::ContextWindowExceeded)),
        "expected structured ContextWindowExceeded, got: {events:?}"
    );
    let error = find_error_message(&events).expect("expected error event");
    assert!(
        error.contains("maximum prompt length"),
        "expected provider message preserved, got: {error}"
    );
}

#[tokio::test]
async fn xai_fallback_tool_ids_use_xai_provider_label() {
    let body = [
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"shell","arguments":""}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"cmd\":\"pwd\"}"}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls","index":0}],"usage":{"prompt_tokens":3,"completion_tokens":2}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let stream_fn = XAiStreamFn::new(server.uri(), "test-key");
    let events = collect_events(&stream_fn).await;
    let tool_call = events.iter().find_map(|event| match event {
        AssistantMessageEvent::ToolCallStart { id, name, .. } => Some((id.clone(), name.clone())),
        _ => None,
    });

    assert_eq!(
        tool_call,
        Some(("xAI-tool-0".to_string(), "shell".to_string()))
    );
}
