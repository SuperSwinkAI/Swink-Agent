//! Shared wiremock fixtures for judge-backed eval tests.
#![allow(dead_code)]

use std::time::Duration;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Canonical POST endpoint used by generic judge fixtures.
pub const DEFAULT_JUDGE_PATH: &str = "/judge";

/// Mount a judge response at the canonical test path.
pub async fn mount_judge_response(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path(DEFAULT_JUDGE_PATH))
        .respond_with(response)
        .mount(server)
        .await;
}

/// Successful JSON verdict body shared across judge-backed tests.
#[must_use]
pub fn verdict_response(
    score: f64,
    pass: bool,
    reason: impl Into<String>,
    label: Option<&str>,
) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "score": score,
        "pass": pass,
        "reason": reason.into(),
        "label": label,
    }))
}

/// Successful verdict response with a fixed delay, used by cancellation tests.
#[must_use]
pub fn delayed_verdict_response(
    score: f64,
    pass: bool,
    reason: impl Into<String>,
    label: Option<&str>,
    delay: Duration,
) -> ResponseTemplate {
    verdict_response(score, pass, reason, label).set_delay(delay)
}

/// Malformed response body used to exercise verdict parsing failures.
#[must_use]
pub fn malformed_response(body: impl Into<String>) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("Content-Type", "application/json")
        .set_body_string(body.into())
}

/// Transport or provider error response.
#[must_use]
pub fn provider_error_response(status: u16, body: impl Into<String>) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_string(body.into())
}

/// 429 fixture carrying a `Retry-After` header for retry tests.
#[must_use]
pub fn rate_limited_response(retry_after_secs: u64) -> ResponseTemplate {
    ResponseTemplate::new(429)
        .insert_header("Retry-After", retry_after_secs.to_string())
        .set_body_string("rate limited")
}
