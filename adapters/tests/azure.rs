#![cfg(feature = "azure")]
//! Wiremock-based tests for `AzureStreamFn`.

mod common;

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use swink_agent::{
    AssistantMessageEvent, ModelSpec, StopReason, StreamErrorKind, StreamFn, StreamOptions,
};
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

// ── Phase 5: User Story 3 — Deployment Routing & Azure AD Auth ────────────

/// Helper: standard SSE body for a simple successful response.
fn simple_sse_body() -> String {
    [
        r#"data: {"choices":[{"delta":{"content":"ok"}}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n")
}

/// Helper: JSON body for a successful token response.
fn token_response_body(access_token: &str, expires_in: u64) -> String {
    serde_json::json!({
        "access_token": access_token,
        "token_type": "Bearer",
        "expires_in": expires_in,
    })
    .to_string()
}

/// Helper: build an `AzureStreamFn` with Entra ID auth pointing at mock servers.
fn entra_stream_fn(api_server: &MockServer, token_url: &str) -> AzureStreamFn {
    AzureStreamFn::new(
        api_server.uri(),
        AzureAuth::EntraId {
            tenant_id: "test-tenant".into(),
            client_id: "test-client-id".into(),
            client_secret: "test-client-secret".into(),
        },
    )
    .with_token_endpoint(token_url.to_string())
}

// ── T032: Entra ID token acquisition — verify POST params ─────────────────

#[tokio::test]
async fn entra_id_token_acquisition_params() {
    let token_server = MockServer::start().await;
    let api_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=client_credentials"))
        .and(body_string_contains("client_id=test-client-id"))
        .and(body_string_contains("client_secret=test-client-secret"))
        .and(body_string_contains("scope=https"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(token_response_body("fresh-token", 3600)),
        )
        .expect(1)
        .mount(&token_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(sse_response(&simple_sse_body()))
        .mount(&api_server)
        .await;

    let token_url = format!("{}/token", token_server.uri());
    let stream_fn = entra_stream_fn(&api_server, &token_url);
    let events = collect_events(&stream_fn).await;

    assert!(events.iter().any(|e| matches!(
        e,
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            ..
        }
    )));
}

// ── T033: Token caching — two requests reuse same token ───────────────────

#[tokio::test]
async fn entra_id_token_caching() {
    let token_server = MockServer::start().await;
    let api_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(token_response_body("cached-token", 3600)),
        )
        .expect(1) // Token endpoint should be called exactly once
        .mount(&token_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(sse_response(&simple_sse_body()))
        .mount(&api_server)
        .await;

    let token_url = format!("{}/token", token_server.uri());
    let stream_fn = entra_stream_fn(&api_server, &token_url);

    // First request
    let events = collect_events(&stream_fn).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::Done { .. }))
    );

    // Second request — should reuse cached token (token endpoint not called again)
    let events = collect_events(&stream_fn).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::Done { .. }))
    );

    // wiremock will verify expect(1) on drop
}

#[tokio::test]
async fn concurrent_entra_id_requests_share_one_token_refresh() {
    let token_server = MockServer::start().await;
    let api_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(token_response_body("deduped-concurrent-token", 3600))
                .set_delay(Duration::from_millis(75)),
        )
        .expect(1)
        .mount(&token_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("Authorization", "Bearer deduped-concurrent-token"))
        .respond_with(sse_response(&simple_sse_body()))
        .expect(2)
        .mount(&api_server)
        .await;

    let token_url = format!("{}/token", token_server.uri());
    let stream_fn = Arc::new(entra_stream_fn(&api_server, &token_url));
    let left = Arc::clone(&stream_fn);
    let right = Arc::clone(&stream_fn);

    let (left_events, right_events) = tokio::join!(
        tokio::spawn(async move { collect_events(left.as_ref()).await }),
        tokio::spawn(async move { collect_events(right.as_ref()).await }),
    );

    for events in [left_events.unwrap(), right_events.unwrap()] {
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AssistantMessageEvent::Done { .. })),
            "expected successful Azure stream events, got: {events:?}"
        );
    }
}

// ── T034: Token refresh — expired token triggers re-acquisition ───────────

#[tokio::test]
async fn entra_id_token_refresh_on_expiry() {
    let token_server = MockServer::start().await;
    let api_server = MockServer::start().await;

    // Return a token that expires immediately (within the refresh margin)
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(200)
                // expires_in=1 means it will be within REFRESH_MARGIN immediately
                .set_body_string(token_response_body("short-lived-token", 1)),
        )
        .expect(2) // Should be called twice since first token expires immediately
        .mount(&token_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(sse_response(&simple_sse_body()))
        .mount(&api_server)
        .await;

    let token_url = format!("{}/token", token_server.uri());
    let stream_fn = entra_stream_fn(&api_server, &token_url);

    // First request — acquires token
    let events = collect_events(&stream_fn).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::Done { .. }))
    );

    // Second request — token is expired (within REFRESH_MARGIN), must re-acquire
    let events = collect_events(&stream_fn).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::Done { .. }))
    );
}

// ── T035: Bearer token appears in Authorization header ─────────��──────────

#[tokio::test]
async fn entra_id_bearer_token_in_auth_header() {
    let token_server = MockServer::start().await;
    let api_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(token_response_body("my-bearer-token", 3600)),
        )
        .mount(&token_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("Authorization", "Bearer my-bearer-token"))
        .respond_with(sse_response(&simple_sse_body()))
        .expect(1)
        .mount(&api_server)
        .await;

    let token_url = format!("{}/token", token_server.uri());
    let stream_fn = entra_stream_fn(&api_server, &token_url);
    let events = collect_events(&stream_fn).await;

    // If the header didn't match, wiremock would return 404 → error event
    assert!(events.iter().any(|e| matches!(
        e,
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            ..
        }
    )));
}

// ── T036: URL constructed as {base_url}/chat/completions ──────────────────

#[tokio::test]
async fn url_construction_with_deployment_path() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/openai/deployments/gpt-4/chat/completions"))
        .and(header("api-key", "test-key"))
        .respond_with(sse_response(&simple_sse_body()))
        .expect(1)
        .mount(&server)
        .await;

    // base_url includes the deployment path
    let auth = AzureAuth::ApiKey("test-key".into());
    let stream_fn = AzureStreamFn::new(format!("{}/openai/deployments/gpt-4", server.uri()), auth);
    let events = collect_events(&stream_fn).await;

    assert!(events.iter().any(|e| matches!(
        e,
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            ..
        }
    )));
}

// ── Phase 6: User Story 4 — Error Handling & Content Filtering ────────────

// ── T040: HTTP 429 → rate-limit error (retryable) ─────────────────────────

#[tokio::test]
async fn http_429_rate_limit_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(429)
                .set_body_string(r#"{"error":{"message":"Rate limit exceeded"}}"#),
        )
        .mount(&server)
        .await;

    let auth = AzureAuth::ApiKey("test-key".into());
    let stream_fn = AzureStreamFn::new(server.uri(), auth);
    let events = collect_events(&stream_fn).await;

    let error_event = events
        .iter()
        .find(|e| matches!(e, AssistantMessageEvent::Error { .. }));
    assert!(
        error_event.is_some(),
        "expected an Error event for HTTP 429"
    );
    match error_event.unwrap() {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(*error_kind, Some(StreamErrorKind::Throttled));
        }
        _ => unreachable!(),
    }
}

// ── T041: HTTP 401 → auth error (not retryable) ───────────────────────────

#[tokio::test]
async fn http_401_auth_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(401).set_body_string(r#"{"error":{"message":"Invalid key"}}"#),
        )
        .mount(&server)
        .await;

    let auth = AzureAuth::ApiKey("bad-key".into());
    let stream_fn = AzureStreamFn::new(server.uri(), auth);
    let events = collect_events(&stream_fn).await;

    let error_event = events
        .iter()
        .find(|e| matches!(e, AssistantMessageEvent::Error { .. }));
    assert!(
        error_event.is_some(),
        "expected an Error event for HTTP 401"
    );
    match error_event.unwrap() {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(*error_kind, Some(StreamErrorKind::Auth));
        }
        _ => unreachable!(),
    }
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::Done { .. })),
        "content_filter should stop the stream without a trailing Done: {events:?}"
    );
}

// ── T042: HTTP 404 → non-retryable error ──────────────────────────────────

#[tokio::test]
async fn http_404_non_retryable_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_string(r#"{"error":{"message":"Deployment not found"}}"#),
        )
        .mount(&server)
        .await;

    let auth = AzureAuth::ApiKey("test-key".into());
    let stream_fn = AzureStreamFn::new(server.uri(), auth);
    let events = collect_events(&stream_fn).await;

    // 404 is a client error, not classified as auth/throttle/network
    let error_event = events
        .iter()
        .find(|e| matches!(e, AssistantMessageEvent::Error { .. }));
    assert!(
        error_event.is_some(),
        "expected an Error event for HTTP 404"
    );
    match error_event.unwrap() {
        AssistantMessageEvent::Error {
            error_kind,
            error_message,
            ..
        } => {
            // 404 has no specific StreamErrorKind — generic client error
            assert_eq!(*error_kind, None);
            assert!(
                error_message.contains("404"),
                "error message should contain status code"
            );
        }
        _ => unreachable!(),
    }
}

// ── T043: HTTP 500 → network error (retryable) ───────────────────────────

#[tokio::test]
async fn http_500_network_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(500).set_body_string(r#"{"error":{"message":"Server error"}}"#),
        )
        .mount(&server)
        .await;

    let auth = AzureAuth::ApiKey("test-key".into());
    let stream_fn = AzureStreamFn::new(server.uri(), auth);
    let events = collect_events(&stream_fn).await;

    let error_event = events
        .iter()
        .find(|e| matches!(e, AssistantMessageEvent::Error { .. }));
    assert!(
        error_event.is_some(),
        "expected an Error event for HTTP 500"
    );
    match error_event.unwrap() {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(*error_kind, Some(StreamErrorKind::Network));
        }
        _ => unreachable!(),
    }
}

// ── T044: SSE stream finish_reason "content_filter" → ContentFiltered ─────

#[tokio::test]
async fn sse_content_filter_finish_reason() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"partial"}}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"content_filter"}],"usage":{"prompt_tokens":5,"completion_tokens":1}}"#,
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

    let error_event = events
        .iter()
        .find(|e| matches!(e, AssistantMessageEvent::Error { .. }));
    assert!(
        error_event.is_some(),
        "expected ContentFiltered error event, got: {events:?}"
    );
    match error_event.unwrap() {
        AssistantMessageEvent::Error {
            error_kind,
            error_message,
            ..
        } => {
            assert_eq!(*error_kind, Some(StreamErrorKind::ContentFiltered));
            assert!(
                error_message.contains("content filter"),
                "message should mention content filter: {error_message}"
            );
        }
        _ => unreachable!(),
    }

    // No Done event should follow — the Error is the sole terminal event (#427)
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

// ── T045: HTTP error body with ContentFilterBlocked → ContentFiltered ──────

#[tokio::test]
async fn http_error_content_filter_blocked() {
    let error_body = serde_json::json!({
        "error": {
            "code": "ContentFilterBlocked",
            "message": "The response was filtered due to the prompt triggering Azure content management policy."
        }
    })
    .to_string();

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_string(&error_body))
        .mount(&server)
        .await;

    let auth = AzureAuth::ApiKey("test-key".into());
    let stream_fn = AzureStreamFn::new(server.uri(), auth);
    let events = collect_events(&stream_fn).await;

    let error_event = events
        .iter()
        .find(|e| matches!(e, AssistantMessageEvent::Error { .. }));
    assert!(
        error_event.is_some(),
        "expected ContentFiltered error event, got: {events:?}"
    );
    match error_event.unwrap() {
        AssistantMessageEvent::Error {
            error_kind,
            error_message,
            ..
        } => {
            assert_eq!(*error_kind, Some(StreamErrorKind::ContentFiltered));
            assert!(
                error_message.contains("content filter"),
                "message should mention content filter: {error_message}"
            );
        }
        _ => unreachable!(),
    }
}

// ── T046: Entra ID token endpoint failure → network error ─────────────────

#[tokio::test]
async fn entra_id_token_endpoint_failure() {
    let token_server = MockServer::start().await;
    let api_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
        .mount(&token_server)
        .await;

    // API server should never be called since token acquisition fails
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(sse_response(&simple_sse_body()))
        .expect(0)
        .mount(&api_server)
        .await;

    let token_url = format!("{}/token", token_server.uri());
    let stream_fn = entra_stream_fn(&api_server, &token_url);
    let events = collect_events(&stream_fn).await;

    assert!(
        matches!(events.first(), Some(AssistantMessageEvent::Start)),
        "pre-stream token failures must start with Start: {events:?}"
    );

    let error_event = events
        .iter()
        .find(|e| matches!(e, AssistantMessageEvent::Error { .. }));
    assert!(
        error_event.is_some(),
        "expected an Error event for token failure, got: {events:?}"
    );
    match error_event.unwrap() {
        AssistantMessageEvent::Error {
            error_kind,
            error_message,
            ..
        } => {
            assert_eq!(*error_kind, Some(StreamErrorKind::Network));
            assert!(
                error_message.contains("token"),
                "error message should mention token: {error_message}"
            );
        }
        _ => unreachable!(),
    }
}
