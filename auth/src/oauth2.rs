//! OAuth2 token refresh helpers.

use serde::Deserialize;
use swink_agent::CredentialError;
use tracing::debug;

/// Response from an OAuth2 token endpoint.
#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    /// The new access token.
    pub access_token: String,
    /// Optional new refresh token (rotation).
    pub refresh_token: Option<String>,
    /// Token lifetime in seconds.
    pub expires_in: Option<i64>,
    /// Token type (usually "Bearer").
    #[serde(default)]
    pub token_type: Option<String>,
}

/// Standard OAuth2 error response body (RFC 6749 §5.2).
///
/// Only the stable `error` code is surfaced from a token endpoint failure.
/// `error_description` is intentionally ignored because providers can place
/// sensitive details there.
#[derive(Debug, Deserialize)]
struct OAuth2ErrorBody {
    error: String,
    #[serde(default)]
    _error_description: Option<String>,
}

/// Build a sanitized refresh-failure reason string from the HTTP status and
/// optional response body.
///
/// The returned string NEVER includes the raw body verbatim. When the body
/// parses as an RFC 6749 §5.2 OAuth2 error response, only the stable `error`
/// code is included. All other bodies are ignored and only the status appears
/// in the surfaced reason.
///
/// The caller may emit redacted metadata such as body length via
/// `tracing::debug!`, but the raw body never reaches
/// `CredentialError::RefreshFailed.reason` and therefore never propagates into
/// tool output.
fn sanitize_refresh_reason(status: reqwest::StatusCode, body: &str) -> String {
    // Attempt to parse a standard OAuth2 error response. Anything else
    // (HTML error pages, opaque vendor JSON, plain text, empty) degrades to a
    // status-only reason.
    if let Ok(parsed) = serde_json::from_str::<OAuth2ErrorBody>(body) {
        format!(
            "token refresh failed: HTTP {} ({})",
            status.as_u16(),
            parsed.error
        )
    } else {
        format!("token refresh failed: HTTP {}", status.as_u16())
    }
}

/// Perform an OAuth2 token refresh via the token endpoint.
///
/// Sends a POST request with `grant_type=refresh_token` to the given
/// `token_url`. Returns the parsed token response on success.
///
/// On failure, the returned [`CredentialError::RefreshFailed`] contains only
/// a sanitized reason: HTTP status plus (if the body is a standard OAuth2
/// error JSON) the stable `error` code. The raw response body is NEVER
/// included in the surfaced error, and debug logs only emit redacted metadata
/// so body contents cannot leak into user-visible tool output.
pub async fn refresh_token(
    client: &reqwest::Client,
    token_url: &str,
    refresh_token: &str,
    client_id: &str,
    client_secret: Option<&str>,
) -> Result<TokenResponse, CredentialError> {
    debug!(token_url, "refreshing OAuth2 token");

    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ];
    if let Some(secret) = client_secret {
        form.push(("client_secret", secret));
    }

    let response = client
        .post(token_url)
        .form(&form)
        .send()
        .await
        .map_err(|e| CredentialError::RefreshFailed {
            key: String::new(),
            reason: format!("HTTP request failed: {e}"),
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        // Raw body stays in debug tracing only; never surfaced in the error
        // reason to avoid leaking token-endpoint payloads into tool output.
        debug!(status = %status, body_len = body.len(), "OAuth2 token refresh failed; response body redacted");
        let reason = sanitize_refresh_reason(status, &body);
        return Err(CredentialError::RefreshFailed {
            key: String::new(),
            reason,
        });
    }

    response
        .json::<TokenResponse>()
        .await
        .map_err(|e| CredentialError::RefreshFailed {
            key: String::new(),
            reason: format!("failed to parse token response: {e}"),
        })
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    use super::*;
    use reqwest::StatusCode;
    use tracing_subscriber::fmt::MakeWriter;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const LEAK_SENTINEL: &str = "LEAK_SENTINEL_ABC123";

    #[derive(Clone, Default)]
    struct SharedLogBuffer(Arc<Mutex<Vec<u8>>>);

    impl SharedLogBuffer {
        fn contents(&self) -> String {
            let bytes = self.0.lock().unwrap().clone();
            String::from_utf8(bytes).unwrap()
        }
    }

    struct SharedLogWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedLogWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for SharedLogBuffer {
        type Writer = SharedLogWriter;

        fn make_writer(&'a self) -> Self::Writer {
            SharedLogWriter(Arc::clone(&self.0))
        }
    }

    #[test]
    fn sanitized_reason_with_standard_oauth2_error_json_ignores_description() {
        let body = format!(
            r#"{{"error":"invalid_grant","error_description":"refresh token expired {LEAK_SENTINEL}"}}"#
        );
        let reason = sanitize_refresh_reason(StatusCode::UNAUTHORIZED, &body);

        assert_eq!(reason, "token refresh failed: HTTP 401 (invalid_grant)");
        assert!(
            !reason.contains(LEAK_SENTINEL),
            "error_description leaked into reason: {reason}"
        );
    }

    #[test]
    fn sanitized_reason_with_oauth2_error_no_description() {
        let body = r#"{"error":"invalid_grant"}"#;
        let reason = sanitize_refresh_reason(StatusCode::BAD_REQUEST, body);
        assert_eq!(
            reason, "token refresh failed: HTTP 400 (invalid_grant)",
            "unexpected reason format"
        );
    }

    #[test]
    fn sanitized_reason_redacts_non_standard_json_body() {
        // Vendor JSON that is NOT an RFC 6749 §5.2 error response — contains
        // a sentinel that must never reach the surfaced reason.
        let body = format!(
            r#"{{"trace_id":"abc","internal_message":"db lookup failed {LEAK_SENTINEL}"}}"#
        );
        let reason = sanitize_refresh_reason(StatusCode::INTERNAL_SERVER_ERROR, &body);

        assert!(
            !reason.contains(LEAK_SENTINEL),
            "sentinel leaked into reason: {reason}"
        );
        assert_eq!(reason, "token refresh failed: HTTP 500");
    }

    #[test]
    fn sanitized_reason_redacts_malformed_body() {
        let body = format!("<html><body>internal error {LEAK_SENTINEL}</body></html>");
        let reason = sanitize_refresh_reason(StatusCode::BAD_GATEWAY, &body);

        assert!(
            !reason.contains(LEAK_SENTINEL),
            "sentinel leaked into reason: {reason}"
        );
        assert!(
            !reason.contains("html"),
            "body fragment leaked into reason: {reason}"
        );
        assert_eq!(reason, "token refresh failed: HTTP 502");
    }

    #[test]
    fn sanitized_reason_redacts_empty_body() {
        let reason = sanitize_refresh_reason(StatusCode::UNAUTHORIZED, "");
        assert_eq!(reason, "token refresh failed: HTTP 401");
    }

    #[test]
    fn sanitized_reason_does_not_include_body_for_ignored_fields() {
        // A standard OAuth2 body with an extra sensitive field outside the
        // recognized schema. serde's default behavior ignores unknown fields,
        // so the sentinel in `debug_info` must NOT appear in the reason.
        let body =
            format!(r#"{{"error":"invalid_grant","debug_info":"raw token dump {LEAK_SENTINEL}"}}"#);
        let reason = sanitize_refresh_reason(StatusCode::UNAUTHORIZED, &body);

        assert!(
            !reason.contains(LEAK_SENTINEL),
            "non-standard field leaked into reason: {reason}"
        );
        assert!(reason.contains("invalid_grant"));
    }

    #[tokio::test]
    async fn refresh_token_debug_log_redacts_response_body() {
        let mock_server = MockServer::start().await;
        let body = format!(
            r#"{{"error":"invalid_grant","error_description":"refresh token expired {LEAK_SENTINEL}"}}"#
        );
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(401).set_body_string(body))
            .mount(&mock_server)
            .await;

        let logs = SharedLogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .without_time()
            .with_writer(logs.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let err = refresh_token(
            &reqwest::Client::new(),
            &format!("{}/token", mock_server.uri()),
            "refresh-token",
            "client-id",
            Some("client-secret"),
        )
        .await
        .unwrap_err();
        let log_output = logs.contents();

        assert!(
            !format!("{err}").contains(LEAK_SENTINEL),
            "refresh error leaked sentinel: {err}"
        );
        assert!(
            !log_output.contains(LEAK_SENTINEL),
            "debug log leaked response body: {log_output}"
        );
        assert!(
            log_output.contains("body_len"),
            "debug log should include body length metadata: {log_output}"
        );
        assert!(
            log_output.contains("response body redacted"),
            "debug log should state the body is redacted: {log_output}"
        );
    }
}
