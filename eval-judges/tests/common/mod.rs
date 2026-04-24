//! Shared wiremock fixtures for `swink-agent-eval-judges`.
#![allow(dead_code, unused_imports)]

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

/// Successful JSON verdict body shared across provider judge tests.
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

/// Canonical verdict JSON string the judge templates are expected to
/// emit inside each provider's response text block.
#[must_use]
pub fn happy_verdict_json() -> String {
    r#"{"score": 0.9, "pass": true, "reason": "looks correct", "label": "equivalent"}"#.to_string()
}

/// Build a 200 response in the Anthropic `/v1/messages` shape wrapping
/// `verdict_text` in a single `text` content block.
#[must_use]
pub fn anthropic_success_body(verdict_text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "id": "msg_01",
        "type": "message",
        "role": "assistant",
        "content": [
            { "type": "text", "text": verdict_text }
        ],
        "model": "claude-test",
        "stop_reason": "end_turn"
    }))
}

/// Build a 200 response in the OpenAI `/v1/chat/completions` shape with
/// a single assistant choice carrying `verdict_text`.
#[must_use]
pub fn openai_success_body(verdict_text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "created": 0,
        "model": "gpt-test",
        "choices": [
            {
                "index": 0,
                "finish_reason": "stop",
                "message": { "role": "assistant", "content": verdict_text }
            }
        ]
    }))
}

/// Build a 200 response in the Gemini `:generateContent` shape with a
/// single candidate/content/parts chain carrying `verdict_text`.
#[must_use]
pub fn gemini_success_body(verdict_text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "candidates": [
            {
                "content": {
                    "role": "model",
                    "parts": [ { "text": verdict_text } ]
                },
                "finishReason": "STOP",
                "index": 0
            }
        ]
    }))
}
