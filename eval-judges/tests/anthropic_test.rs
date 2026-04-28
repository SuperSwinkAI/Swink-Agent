//! Wiremock-backed integration tests for [`AnthropicJudgeClient`].
//!
//! Coverage per T037 (spec 043-evals-adv-features):
//!
//! * `anthropic_happy_path` — valid verdict round-trips.
//! * `anthropic_rate_limit_absorbed_by_retry` — a single 429 blip is
//!   followed by a 200; the shared retry helper (T047) absorbs it.
//! * `anthropic_exhausted_retries_surface_transport` — persistent 429
//!   drains the retry budget and surfaces as `JudgeError::Transport`.
//! * `anthropic_malformed_response_is_terminal` — a 200 with a body that
//!   cannot parse as a verdict JSON surfaces as
//!   `JudgeError::MalformedResponse` on the *first* attempt.
//! * `anthropic_cancellation_token_short_circuits` — cancellation surfaces
//!   as `JudgeError::Other("cancelled")` before the provider is called.

#![cfg(feature = "anthropic")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use swink_agent_eval::judge::{JudgeClient, JudgeError, RetryPolicy};
use swink_agent_eval_judges::AnthropicJudgeClient;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

mod common;
use common::{anthropic_success_body, happy_verdict_json};

fn test_policy(max_attempts: u32) -> RetryPolicy {
    // Tight schedule so `max_attempts` attempts finish inside the test's
    // default 60s harness budget even under the 4-minute production cap.
    RetryPolicy::new(max_attempts, Duration::from_millis(10), false)
}

#[tokio::test]
async fn anthropic_happy_path() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(anthropic_success_body(&happy_verdict_json()))
        .expect(1)
        .mount(&server)
        .await;

    let client = AnthropicJudgeClient::new(server.uri(), "test-key", "claude-test")
        .with_retry_policy(test_policy(6));

    let verdict = client.judge("grade this").await.expect("verdict");
    assert!((verdict.score - 0.9).abs() < f64::EPSILON);
    assert!(verdict.pass);
    assert_eq!(verdict.reason.as_deref(), Some("looks correct"));
    assert_eq!(verdict.label.as_deref(), Some("equivalent"));
}

struct OneBlipThenSuccess {
    count: Arc<AtomicUsize>,
    success: ResponseTemplate,
}

impl Respond for OneBlipThenSuccess {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let n = self.count.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            ResponseTemplate::new(429).set_body_string("slow down")
        } else {
            self.success.clone()
        }
    }
}

#[tokio::test]
async fn anthropic_rate_limit_absorbed_by_retry() {
    let server = MockServer::start().await;
    let count = Arc::new(AtomicUsize::new(0));

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(OneBlipThenSuccess {
            count: count.clone(),
            success: anthropic_success_body(&happy_verdict_json()),
        })
        .expect(2)
        .mount(&server)
        .await;

    let client = AnthropicJudgeClient::new(server.uri(), "test-key", "claude-test")
        .with_retry_policy(test_policy(6));

    let verdict = client
        .judge("grade this")
        .await
        .expect("verdict after retry");
    assert!(verdict.pass);
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn anthropic_exhausted_retries_surface_transport() {
    let server = MockServer::start().await;

    // The retry policy in the client has `max_attempts = 3`, so the
    // provider should see exactly 3 calls before the helper gives up.
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(429).set_body_string("slow down"))
        .expect(3)
        .mount(&server)
        .await;

    let client = AnthropicJudgeClient::new(server.uri(), "test-key", "claude-test")
        .with_retry_policy(test_policy(3));

    let err = client
        .judge("grade this")
        .await
        .expect_err("must exhaust retries");
    match err {
        JudgeError::Transport(msg) => {
            assert!(
                msg.contains("429"),
                "expected 429 in transport error, got: {msg}"
            );
        }
        other => panic!("expected Transport, got {other:?}"),
    }
}

#[tokio::test]
async fn anthropic_malformed_response_is_terminal() {
    let server = MockServer::start().await;

    // 200 OK with a text block that does not parse as the verdict JSON.
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(anthropic_success_body("not a verdict at all"))
        .expect(1)
        .mount(&server)
        .await;

    let client = AnthropicJudgeClient::new(server.uri(), "test-key", "claude-test")
        .with_retry_policy(test_policy(6));

    let err = client
        .judge("grade this")
        .await
        .expect_err("must surface malformed");
    assert!(matches!(err, JudgeError::MalformedResponse(_)));
}

#[tokio::test]
async fn anthropic_cancellation_token_short_circuits() {
    // No mock mounted: if the client contacted the server at all, the
    // test would fail via unrequested-path on teardown.
    let server = MockServer::start().await;

    let cancel = CancellationToken::new();
    cancel.cancel();

    let client = AnthropicJudgeClient::new(server.uri(), "test-key", "claude-test")
        .with_retry_policy(test_policy(6))
        .with_cancellation(cancel);

    let err = client
        .judge("grade this")
        .await
        .expect_err("cancelled before dispatch");
    match err {
        JudgeError::Other(msg) => assert!(msg.contains("cancel")),
        other => panic!("expected cancellation, got {other:?}"),
    }

    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        0
    );
}

#[tokio::test]
async fn anthropic_blocking_wrapper_delegates_to_async() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(anthropic_success_body(&happy_verdict_json()))
        .expect(1)
        .mount(&server)
        .await;

    let client = AnthropicJudgeClient::new(server.uri(), "test-key", "claude-test")
        .with_retry_policy(test_policy(3));
    let blocking = swink_agent_eval_judges::BlockingAnthropicJudgeClient::new(client);

    // Dispatch the blocking call on a spawned thread so
    // `Handle::current().block_on` inside the wrapper doesn't try to
    // block the current runtime worker.
    let verdict = tokio::task::spawn_blocking(move || blocking.judge("grade this"))
        .await
        .expect("join")
        .expect("verdict");
    assert!(verdict.pass);
}
