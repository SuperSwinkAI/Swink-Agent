//! Tests for `azure`.
#![cfg(test)]

use super::*;

#[test]
fn classify_429_is_transport() {
    let error = classify_http_error(StatusCode::TOO_MANY_REQUESTS, "slow down");
    assert!(matches!(error, JudgeError::Transport(_)));
}

#[test]
fn extract_verdict_requires_choice_content() {
    let error = extract_verdict(&AzureResponse {
        choices: vec![AzureChoice {
            message: AzureChoiceMessage { content: None },
        }],
    })
    .expect_err("missing content must fail");
    assert!(matches!(error, JudgeError::MalformedResponse(_)));
}
