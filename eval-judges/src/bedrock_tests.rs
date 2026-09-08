//! Tests for `bedrock`.
#![cfg(test)]

use super::*;

#[test]
fn classify_429_is_transport() {
    let error = classify_http_error(StatusCode::TOO_MANY_REQUESTS, "slow");
    assert!(matches!(error, JudgeError::Transport(_)));
}

#[test]
fn classify_500_is_transport() {
    let error = classify_http_error(StatusCode::INTERNAL_SERVER_ERROR, "boom");
    assert!(matches!(error, JudgeError::Transport(_)));
}

#[test]
fn classify_400_is_terminal() {
    let error = classify_http_error(StatusCode::BAD_REQUEST, "nope");
    assert!(matches!(error, JudgeError::Other(_)));
}

#[test]
fn urlencode_colon_in_model_id() {
    let encoded = urlencode_path("anthropic.claude-3-5-sonnet-20240620-v1:0");
    assert!(encoded.contains("%3A"));
    assert!(!encoded.contains(':'));
}
