#![cfg(feature = "azure")]
//! Wiremock-based tests for `AzureStreamFn`.

mod common;

use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer};

use swink_agent::{AssistantMessageEvent, ModelSpec, StopReason, StreamFn, StreamOptions};
use swink_agent_adapters::{AzureAuth, AzureStreamFn};

use common::{sse_response, test_context};

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
