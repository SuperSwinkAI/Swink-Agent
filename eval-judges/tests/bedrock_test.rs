//! Wiremock-backed integration tests for [`BedrockJudgeClient`].
//!
//! Coverage mirrors the existing judge clients: happy path, retryable
//! 429 recovery, exhausted retries, malformed verdict, cancellation, and
//! the blocking wrapper.

#![cfg(feature = "bedrock")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use swink_agent_eval::judge::{JudgeClient, JudgeError, RetryPolicy};
use swink_agent_eval_judges::BedrockJudgeClient;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

mod common;
use common::{anthropic_success_body, happy_verdict_json};

const TEST_MODEL: &str = "anthropic.claude-3-5-sonnet-20240620-v1:0";
const TEST_MODEL_ENCODED: &str = "anthropic.claude-3-5-sonnet-20240620-v1%3A0";

fn test_policy(max_attempts: u32) -> RetryPolicy {
    RetryPolicy::new(max_attempts, Duration::from_millis(10), false)
}

fn invoke_path() -> String {
    format!("/model/{TEST_MODEL_ENCODED}/invoke")
}

#[tokio::test]
async fn bedrock_happy_path() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(invoke_path()))
        .respond_with(anthropic_success_body(&happy_verdict_json()))
        .expect(1)
        .mount(&server)
        .await;

    let client = BedrockJudgeClient::new(server.uri(), "test-key", TEST_MODEL)
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
async fn bedrock_rate_limit_absorbed_by_retry() {
    let server = MockServer::start().await;
    let count = Arc::new(AtomicUsize::new(0));

    Mock::given(method("POST"))
        .and(path(invoke_path()))
        .respond_with(OneBlipThenSuccess {
            count: count.clone(),
            success: anthropic_success_body(&happy_verdict_json()),
        })
        .expect(2)
        .mount(&server)
        .await;

    let client = BedrockJudgeClient::new(server.uri(), "test-key", TEST_MODEL)
        .with_retry_policy(test_policy(6));

    let verdict = client
        .judge("grade this")
        .await
        .expect("verdict after retry");
    assert!(verdict.pass);
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn bedrock_exhausted_retries_surface_transport() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path(invoke_path()))
        .respond_with(ResponseTemplate::new(429).set_body_string("slow down"))
        .expect(3)
        .mount(&server)
        .await;

    let client = BedrockJudgeClient::new(server.uri(), "test-key", TEST_MODEL)
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
async fn bedrock_malformed_response_is_terminal() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path(invoke_path()))
        .respond_with(anthropic_success_body("not a verdict at all"))
        .expect(1)
        .mount(&server)
        .await;

    let client = BedrockJudgeClient::new(server.uri(), "test-key", TEST_MODEL)
        .with_retry_policy(test_policy(6));

    let error = client
        .judge("grade this")
        .await
        .expect_err("must surface malformed");
    assert!(matches!(error, JudgeError::MalformedResponse(_)));
}

#[tokio::test]
async fn bedrock_cancellation_token_short_circuits() {
    let server = MockServer::start().await;

    let cancel = CancellationToken::new();
    cancel.cancel();

    let client = BedrockJudgeClient::new(server.uri(), "test-key", TEST_MODEL)
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
async fn bedrock_blocking_wrapper_delegates_to_async() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(invoke_path()))
        .respond_with(anthropic_success_body(&happy_verdict_json()))
        .expect(1)
        .mount(&server)
        .await;

    let client = BedrockJudgeClient::new(server.uri(), "test-key", TEST_MODEL)
        .with_retry_policy(test_policy(3));
    let blocking = swink_agent_eval_judges::BlockingBedrockJudgeClient::new(client);

    let verdict = tokio::task::spawn_blocking(move || blocking.judge("grade this"))
        .await
        .expect("join")
        .expect("verdict");
    assert!(verdict.pass);
}
