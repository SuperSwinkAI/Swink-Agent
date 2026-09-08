//! Tests for `gemini`.
#![cfg(test)]

use super::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn classify_429_is_transport() {
    let error = classify_http_error(StatusCode::TOO_MANY_REQUESTS, "slow");
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

#[tokio::test]
async fn transport_errors_do_not_expose_api_key() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    drop(listener);

    let api_key = "secret-key-from-issue-983";
    let client = GeminiJudgeClient::new(base_url, api_key, "gemini-1.5-flash")
        .with_retry_policy(RetryPolicy::new(1, Duration::from_millis(1), false));

    let error = client.judge("grade this").await.expect_err("request fails");
    assert!(matches!(error, JudgeError::Transport(_)));

    let display = error.to_string();
    let debug = format!("{error:?}");
    assert!(!display.contains(api_key), "{display}");
    assert!(!debug.contains(api_key), "{debug}");
}

#[tokio::test]
async fn body_parse_errors_do_not_expose_api_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-1.5-flash:generateContent"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{"))
        .expect(1)
        .mount(&server)
        .await;

    let api_key = "secret-json-key-from-issue-983";
    let client = GeminiJudgeClient::new(server.uri(), api_key, "gemini-1.5-flash")
        .with_retry_policy(RetryPolicy::new(1, Duration::from_millis(1), false));

    let error = client
        .judge("grade this")
        .await
        .expect_err("body parse fails");
    assert!(matches!(error, JudgeError::MalformedResponse(_)));

    let display = error.to_string();
    let debug = format!("{error:?}");
    assert!(!display.contains(api_key), "{display}");
    assert!(!debug.contains(api_key), "{debug}");
}
