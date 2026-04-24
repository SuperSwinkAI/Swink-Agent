//! Wiremock-backed integration tests for [`AzureJudgeClient`].
//!
//! Coverage per T043: happy path, 429 retry, exhausted retries, malformed
//! verdict, cancellation, and the blocking wrapper.

#![cfg(feature = "azure")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use swink_agent_eval::judge::{JudgeClient, JudgeError, RetryPolicy};
use swink_agent_eval_judges::AzureJudgeClient;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

mod common;
use common::{happy_verdict_json, openai_success_body};

fn test_policy(max_attempts: u32) -> RetryPolicy {
    RetryPolicy::new(max_attempts, Duration::from_millis(10), false)
}

#[tokio::test]
async fn azure_happy_path() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/openai/deployments/gpt-4o/chat/completions"))
        .and(query_param("api-version", "2024-10-21"))
        .and(header("api-key", "test-key"))
        .respond_with(openai_success_body(&happy_verdict_json()))
        .expect(1)
        .mount(&server)
        .await;

    let client = AzureJudgeClient::new(
        format!("{}/openai/deployments/gpt-4o", server.uri()),
        "test-key",
        "gpt-4o",
    )
    .with_retry_policy(test_policy(6));

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
            ResponseTemplate::new(429).set_body_string("slow down")
        } else {
            self.success.clone()
        }
    }
}

#[tokio::test]
async fn azure_rate_limit_absorbed_by_retry() {
    let server = MockServer::start().await;
    let count = Arc::new(AtomicUsize::new(0));

    Mock::given(method("POST"))
        .and(path("/openai/deployments/gpt-4o/chat/completions"))
        .and(query_param("api-version", "2024-10-21"))
        .respond_with(OneBlipThenSuccess {
            count: count.clone(),
            success: openai_success_body(&happy_verdict_json()),
        })
        .expect(2)
        .mount(&server)
        .await;

    let client = AzureJudgeClient::new(
        format!("{}/openai/deployments/gpt-4o", server.uri()),
        "test-key",
        "gpt-4o",
    )
    .with_retry_policy(test_policy(6));

    let verdict = client
        .judge("grade this")
        .await
        .expect("verdict after retry");
    assert!(verdict.pass);
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn azure_exhausted_retries_surface_transport() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/openai/deployments/gpt-4o/chat/completions"))
        .and(query_param("api-version", "2024-10-21"))
        .respond_with(ResponseTemplate::new(429).set_body_string("slow down"))
        .expect(3)
        .mount(&server)
        .await;

    let client = AzureJudgeClient::new(
        format!("{}/openai/deployments/gpt-4o", server.uri()),
        "test-key",
        "gpt-4o",
    )
    .with_retry_policy(test_policy(3));

    let error = client
        .judge("grade this")
        .await
        .expect_err("must exhaust retries");
    match error {
        JudgeError::Transport(message) => assert!(message.contains("429")),
        other => panic!("expected Transport, got {other:?}"),
    }
}

#[tokio::test]
async fn azure_malformed_response_is_terminal() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/openai/deployments/gpt-4o/chat/completions"))
        .and(query_param("api-version", "2024-10-21"))
        .respond_with(openai_success_body("not a verdict at all"))
        .expect(1)
        .mount(&server)
        .await;

    let client = AzureJudgeClient::new(
        format!("{}/openai/deployments/gpt-4o", server.uri()),
        "test-key",
        "gpt-4o",
    )
    .with_retry_policy(test_policy(6));

    let error = client
        .judge("grade this")
        .await
        .expect_err("must surface malformed");
    assert!(matches!(error, JudgeError::MalformedResponse(_)));
}

#[tokio::test]
async fn azure_cancellation_token_short_circuits() {
    let server = MockServer::start().await;

    let cancel = CancellationToken::new();
    cancel.cancel();

    let client = AzureJudgeClient::new(
        format!("{}/openai/deployments/gpt-4o", server.uri()),
        "test-key",
        "gpt-4o",
    )
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
async fn azure_blocking_wrapper_delegates_to_async() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/openai/deployments/gpt-4o/chat/completions"))
        .and(query_param("api-version", "2024-10-21"))
        .respond_with(openai_success_body(&happy_verdict_json()))
        .expect(1)
        .mount(&server)
        .await;

    let client = AzureJudgeClient::new(
        format!("{}/openai/deployments/gpt-4o", server.uri()),
        "test-key",
        "gpt-4o",
    )
    .with_retry_policy(test_policy(3));
    let blocking = swink_agent_eval_judges::BlockingAzureJudgeClient::new(client);

    let verdict = tokio::task::spawn_blocking(move || blocking.judge("grade this"))
        .await
        .expect("join")
        .expect("verdict");
    assert!(verdict.pass);
}
