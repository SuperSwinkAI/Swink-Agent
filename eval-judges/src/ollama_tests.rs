//! Tests for `ollama`.
#![cfg(test)]

use super::*;

#[test]
fn classify_429_is_transport() {
    let error = classify_http_error(StatusCode::TOO_MANY_REQUESTS, "slow down");
    assert!(matches!(error, JudgeError::Transport(_)));
}

#[test]
fn classify_503_is_transport() {
    let error = classify_http_error(StatusCode::SERVICE_UNAVAILABLE, "down");
    assert!(matches!(error, JudgeError::Transport(_)));
}

#[test]
fn classify_401_is_terminal() {
    let error = classify_http_error(StatusCode::UNAUTHORIZED, "bad key");
    assert!(matches!(error, JudgeError::Other(_)));
}
