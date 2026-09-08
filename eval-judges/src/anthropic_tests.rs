//! Tests for `anthropic`.
#![cfg(test)]

use super::*;

#[test]
fn classify_429_is_transport() {
    let err = classify_http_error(StatusCode::TOO_MANY_REQUESTS, "rate");
    assert!(matches!(err, JudgeError::Transport(_)));
}

#[test]
fn classify_500_is_transport() {
    let err = classify_http_error(StatusCode::INTERNAL_SERVER_ERROR, "boom");
    assert!(matches!(err, JudgeError::Transport(_)));
}

#[test]
fn classify_400_is_terminal() {
    let err = classify_http_error(StatusCode::BAD_REQUEST, "nope");
    assert!(matches!(err, JudgeError::Other(_)));
}
