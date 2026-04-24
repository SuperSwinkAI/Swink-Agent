//! Wiremock-backed integration tests for [`OllamaJudgeClient`].
//!
//! Coverage per T045: happy path, 503 retry, exhausted retries, malformed
//! verdict, cancellation, and the blocking wrapper.

#![cfg(feature = "ollama")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use swink_agent_eval::judge::{JudgeClient, JudgeError, RetryPolicy};
use swink_agent_eval_judges::OllamaJudgeClient;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

mod common;
use common::happy_verdict_json;

fn test_policy(max_attempts: u32) -> RetryPolicy {
    RetryPolicy::new(max_attempts, Duration::from_millis(10), false)
}

/// Build a 200 response in the Ollama `/api/chat` shape wrapping
/// `verdict_text` in `message.content`.
fn ollama_success_body(verdict_text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "model": "llama3",
        "created_at": "2024-01-01T00:00:00Z",
        "message": {
            "role": "assistant",
            "content": verdict_text
        },
        "done": true
    }))
}

#[tokio::test]
async fn ollama_happy_path() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ollama_success_body(&happy_verdict_json()))
        .expect(1)
        .mount(&server)
        .await;

    let client = OllamaJudgeClient::new(server.uri(), "llama3").with_retry_policy(test_policy(6));

    let verdict = client.judge("grade this").await.expect("verdict");
    assert!(verdict.pass);
    assert!((verdict.score - 0.9).abs() < f64::EPSILON);
    assert_eq!(verdict.reason.as_deref(), Some("looks correct"));
}

struct OneBlipThenSuccess {
    count: Arc<AtomicUsize>,
    success: ResponseTemplate,
}

impl Respond for OneBlipThenSuccess {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let call_count = self.count.fetch_add(1, Ordering::SeqCst);
        if call_count == 0 {
            ResponseTemplate::new(503).set_body_string("service unavailable")
        } else {
            self.success.clone()
        }
    }
}

#[tokio::test]
async fn ollama_rate_limit_absorbed_by_retry() {
    let server = MockServer::start().await;
    let count = Arc::new(AtomicUsize::new(0));

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(OneBlipThenSuccess {
            count: count.clone(),
            success: ollama_success_body(&happy_verdict_json()),
        })
        .expect(2)
        .mount(&server)
        .await;

    let client = OllamaJudgeClient::new(server.uri(), "llama3").with_retry_policy(test_policy(6));

    let verdict = client
        .judge("grade this")
        .await
        .expect("verdict after retry");
    assert!(verdict.pass);
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn ollama_exhausted_retries_surface_transport() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(503).set_body_string("service unavailable"))
        .expect(3)
        .mount(&server)
        .await;

    let client = OllamaJudgeClient::new(server.uri(), "llama3").with_retry_policy(test_policy(3));

    let error = client
        .judge("grade this")
        .await
        .expect_err("must exhaust retries");
    match error {
        JudgeError::Transport(message) => assert!(message.contains("503")),
        other => panic!("expected Transport, got {other:?}"),
    }
}

#[tokio::test]
async fn ollama_malformed_response_is_terminal() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ollama_success_body("not a verdict at all"))
        .expect(1)
        .mount(&server)
        .await;

    let client = OllamaJudgeClient::new(server.uri(), "llama3").with_retry_policy(test_policy(6));

    let error = client
        .judge("grade this")
        .await
        .expect_err("must surface malformed");
    assert!(matches!(error, JudgeError::MalformedResponse(_)));
}

#[tokio::test]
async fn ollama_cancellation_token_short_circuits() {
    let server = MockServer::start().await;

    let cancel = CancellationToken::new();
    cancel.cancel();

    let client = OllamaJudgeClient::new(server.uri(), "llama3")
        .with_retry_policy(test_policy(6))
        .with_cancellation(cancel);

    let error = client
        .judge("grade this")
        .await
        .expect_err("cancelled before dispatch");
    match error {
        JudgeError::Other(message) => assert!(message.contains("cancel")),
        other => panic!("expected cancellation, got {other:?}"),
    }

    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        0
    );
}

#[tokio::test]
async fn ollama_blocking_wrapper_delegates_to_async() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ollama_success_body(&happy_verdict_json()))
        .expect(1)
        .mount(&server)
        .await;

    let client = OllamaJudgeClient::new(server.uri(), "llama3").with_retry_policy(test_policy(3));
    let blocking = swink_agent_eval_judges::BlockingOllamaJudgeClient::new(client);

    let verdict = tokio::task::spawn_blocking(move || blocking.judge("grade this"))
        .await
        .expect("join")
        .expect("verdict");
    assert!(verdict.pass);
}
