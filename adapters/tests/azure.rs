#![cfg(feature = "azure")]
//! Wiremock-based tests for `AzureStreamFn`.

mod common;

use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer};

use swink_agent::{AssistantMessageEvent, ModelSpec, StopReason, StreamFn, StreamOptions};
use swink_agent_adapters::{AzureAuth, AzureStreamFn};

use common::{event_name, sse_response, test_context};

fn test_model() -> ModelSpec {
    ModelSpec::new("azure", "gpt-5.4")
}

async fn collect_events(stream_fn: &AzureStreamFn) -> Vec<AssistantMessageEvent> {
    let token = CancellationToken::new();
    stream_fn
        .stream(
            &test_model(),
            &test_context(),
            &StreamOptions::default(),
            token,
        )
        .collect::<Vec<_>>()
        .await
}

// ── T019: Text streaming with API key auth ──────────────────────────────────

#[tokio::test]
async fn text_stream_with_api_key_auth() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"hello"}}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"content":" world"}}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":2}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/openai/v1/chat/completions"))
        .and(header("api-key", "test-key"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let auth = AzureAuth::ApiKey("test-key".into());
    let stream_fn = AzureStreamFn::new(format!("{}/openai/v1", server.uri()), auth);
    let events = collect_events(&stream_fn).await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::Start))
    );

    // Verify text deltas arrive incrementally
    let deltas: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas, vec!["hello", " world"]);

    assert!(events.iter().any(|e| matches!(
        e,
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            ..
        }
    )));
}

// ── T020: Verify api-key header is set correctly ────────────────────────────

#[tokio::test]
async fn api_key_header_set_correctly() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"ok"}}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("api-key", "my-secret-key"))
        .respond_with(sse_response(&body))
        .expect(1)
        .mount(&server)
        .await;

    let auth = AzureAuth::ApiKey("my-secret-key".into());
    let stream_fn = AzureStreamFn::new(server.uri(), auth);
    let events = collect_events(&stream_fn).await;

    // If the header didn't match, wiremock would return 404 and we'd get an error event.
    assert!(events.iter().any(|e| matches!(
        e,
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            ..
        }
    )));
}

// ── T021: Trailing slash in base URL is stripped ────────────────────────────

#[tokio::test]
async fn trailing_slash_stripped_from_base_url() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"ok"}}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("api-key", "test-key"))
        .respond_with(sse_response(&body))
        .expect(1)
        .mount(&server)
        .await;

    // Pass URL with trailing slash — should still work
    let auth = AzureAuth::ApiKey("test-key".into());
    let stream_fn = AzureStreamFn::new(format!("{}/", server.uri()), auth);
    let events = collect_events(&stream_fn).await;

    assert!(events.iter().any(|e| matches!(
        e,
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            ..
        }
    )));
}

// ── T022: [DONE] sentinel produces terminal Done event ──────────────────────

#[tokio::test]
async fn done_sentinel_produces_terminal_event() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let auth = AzureAuth::ApiKey("test-key".into());
    let stream_fn = AzureStreamFn::new(server.uri(), auth);
    let events = collect_events(&stream_fn).await;

    // The last meaningful event should be Done
    let last_event = events.last().expect("should have events");
    assert!(
        matches!(
            last_event,
            AssistantMessageEvent::Done {
                stop_reason: StopReason::Stop,
                ..
            }
        ),
        "expected Done as terminal event, got: {last_event:?}"
    );
}

// ── T012: AzureAuth Debug redacts secrets ───────────────────────────────────

#[test]
fn azure_auth_debug_redacts_secrets() {
    let api_key = AzureAuth::ApiKey("super-secret".into());
    let debug_str = format!("{api_key:?}");
    assert!(
        !debug_str.contains("super-secret"),
        "API key should be redacted"
    );
    assert!(debug_str.contains("[REDACTED]"));

    let entra = AzureAuth::EntraId {
        tenant_id: "tid".into(),
        client_id: "cid".into(),
        client_secret: "csecret".into(),
    };
    let debug_str = format!("{entra:?}");
    assert!(!debug_str.contains("tid"), "tenant_id should be redacted");
    assert!(!debug_str.contains("cid"), "client_id should be redacted");
    assert!(
        !debug_str.contains("csecret"),
        "client_secret should be redacted"
    );
}

// ── T016: AzureStreamFn Debug redacts credentials ───────────────────────────

#[test]
fn azure_stream_fn_debug_redacts_credentials() {
    let stream_fn = AzureStreamFn::new(
        "https://myresource.openai.azure.com/openai/deployments/gpt-4",
        AzureAuth::ApiKey("my-key".into()),
    );
    let debug_str = format!("{stream_fn:?}");
    assert!(
        !debug_str.contains("my-key"),
        "API key should be redacted in Debug"
    );
    assert!(
        debug_str.contains("myresource"),
        "base_url should be visible"
    );
}

// ── Phase 4: User Story 2 — Tool Call Streaming ────────────────────────────

// ── T024: Tool call streaming — verify ToolCallStart, ToolCallDelta, ToolCallEnd ──

#[tokio::test]
async fn tool_call_stream_events() {
    let body = [
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc123","function":{"name":"bash","arguments":""}}]},"index":0}]}"#,
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
        .and(path("/chat/completions"))
        .and(header("api-key", "test-key"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let auth = AzureAuth::ApiKey("test-key".into());
    let stream_fn = AzureStreamFn::new(server.uri(), auth);
    let events = collect_events(&stream_fn).await;

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
    assert_eq!(start, Some(("call_abc123".to_string(), "bash".to_string())));

    // Verify stop reason is ToolUse
    let done = events.iter().find_map(|e| match e {
        AssistantMessageEvent::Done { stop_reason, .. } => Some(*stop_reason),
        _ => None,
    });
    assert_eq!(done, Some(StopReason::ToolUse));
}

// ── T025: Multiple parallel tool calls — separate indexed blocks ────────────

#[tokio::test]
async fn multiple_parallel_tool_calls() {
    let body = [
        // First tool call starts
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_first","function":{"name":"bash","arguments":""}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"cmd\":\"ls\"}"}}]},"index":0}]}"#,
        "",
        // Second tool call starts
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call_second","function":{"name":"read_file","arguments":""}}]},"index":0}]}"#,
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
        .and(path("/chat/completions"))
        .and(header("api-key", "test-key"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let auth = AzureAuth::ApiKey("test-key".into());
    let stream_fn = AzureStreamFn::new(server.uri(), auth);
    let events = collect_events(&stream_fn).await;

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
    assert_eq!(tool_starts[0].2, "bash");
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

// ── T026: Tool call arguments form valid JSON upon ToolCallEnd ──────────────

#[tokio::test]
async fn tool_call_arguments_form_valid_json() {
    let body = [
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_json","function":{"name":"write_file","arguments":""}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":"}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"out.txt\","}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"content\":"}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"hello world\"}"}}]},"index":0}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls","index":0}],"usage":{"prompt_tokens":8,"completion_tokens":15}}"#,
        "",
        "data: [DONE]",
        "",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("api-key", "test-key"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let auth = AzureAuth::ApiKey("test-key".into());
    let stream_fn = AzureStreamFn::new(server.uri(), auth);
    let events = collect_events(&stream_fn).await;

    // Collect all argument deltas
    let arguments: String = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::ToolCallDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();

    // Verify combined arguments form valid JSON
    let parsed: serde_json::Value =
        serde_json::from_str(&arguments).expect("tool call arguments should be valid JSON");
    assert_eq!(parsed["path"], "out.txt");
    assert_eq!(parsed["content"], "hello world");
}
