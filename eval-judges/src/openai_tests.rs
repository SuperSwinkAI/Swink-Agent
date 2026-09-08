//! Tests for `openai`.
#![cfg(test)]

use super::*;

#[test]
fn classify_429_is_transport() {
    let err = classify_http_error(StatusCode::TOO_MANY_REQUESTS, "rate");
    assert!(matches!(err, JudgeError::Transport(_)));
}

#[test]
fn classify_503_is_transport() {
    let err = classify_http_error(StatusCode::SERVICE_UNAVAILABLE, "down");
    assert!(matches!(err, JudgeError::Transport(_)));
}

#[test]
fn classify_401_is_terminal() {
    let err = classify_http_error(StatusCode::UNAUTHORIZED, "bad key");
    assert!(matches!(err, JudgeError::Other(_)));
}
