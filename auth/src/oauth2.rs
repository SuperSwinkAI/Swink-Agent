//! OAuth2 token refresh helpers.

use std::fmt;

use serde::Deserialize;
use swink_agent::CredentialError;
use tracing::debug;

/// Response from an OAuth2 token endpoint.
#[derive(Deserialize)]
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

impl fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenResponse")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_in", &self.expires_in)
            .field("token_type", &self.token_type)
            .finish()
    }
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

/// Build a sanitized token-endpoint failure reason string from the HTTP
/// status and optional response body, for the given `action` (e.g. `"token
/// refresh"`, `"authorization code exchange"`).
///
/// The returned string NEVER includes the raw body verbatim. When the body
/// parses as an RFC 6749 §5.2 OAuth2 error response, only the stable `error`
/// code is included. All other bodies are ignored and only the status appears
/// in the surfaced reason.
///
/// The caller may emit redacted metadata such as body length via
/// `tracing::debug!`, but the raw body never reaches a surfaced
/// `CredentialError` reason and therefore never propagates into tool output.
fn sanitize_oauth2_error_reason(action: &str, status: reqwest::StatusCode, body: &str) -> String {
    // Attempt to parse a standard OAuth2 error response. Anything else
    // (HTML error pages, opaque vendor JSON, plain text, empty) degrades to a
    // status-only reason.
    if let Ok(parsed) = serde_json::from_str::<OAuth2ErrorBody>(body) {
        format!(
            "{action} failed: HTTP {} ({})",
            status.as_u16(),
            parsed.error
        )
    } else {
        format!("{action} failed: HTTP {}", status.as_u16())
    }
}

/// Sanitized reason for a failed `refresh_token` call. Thin wrapper over
/// [`sanitize_oauth2_error_reason`] preserving the historical `"token
/// refresh failed: ..."` wording.
fn sanitize_refresh_reason(status: reqwest::StatusCode, body: &str) -> String {
    sanitize_oauth2_error_reason("token refresh", status, body)
}

/// Sanitized reason for a failed `exchange_code` call.
fn sanitize_code_exchange_reason(status: reqwest::StatusCode, body: &str) -> String {
    sanitize_oauth2_error_reason("authorization code exchange", status, body)
}

fn sanitize_token_endpoint(token_url: &str) -> String {
    match reqwest::Url::parse(token_url) {
        Ok(url) => {
            let mut endpoint = format!("{}://", url.scheme());
            endpoint.push_str(url.host_str().unwrap_or("<unknown-host>"));
            if let Some(port) = url.port() {
                endpoint.push(':');
                endpoint.push_str(&port.to_string());
            }
            if url.path() == "/" {
                endpoint.push('/');
            } else {
                endpoint.push_str("/<path>");
            }
            endpoint
        }
        Err(_) => "invalid-url".to_string(),
    }
}

fn sanitize_transport_reason(action: &str, error: &reqwest::Error) -> String {
    let kind = if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect failure"
    } else if error.is_request() {
        "request failure"
    } else if error.is_body() {
        "body failure"
    } else if error.is_decode() {
        "decode failure"
    } else {
        "failure"
    };
    format!("{action} failed: transport {kind}")
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
    debug!(
        token_endpoint = %sanitize_token_endpoint(token_url),
        "refreshing OAuth2 token"
    );

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
            reason: sanitize_transport_reason("token refresh", &e),
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        // Raw body stays in debug tracing only; never surfaced in the error
        // reason to avoid leaking token-endpoint payloads into tool output.
        // Include the sanitized endpoint here in addition to the initial
        // "refreshing" event: the initial event can be dropped when the
        // default subscriber isn't attached to reqwest's worker thread,
        // but this one always fires on the test's thread after `.await`.
        debug!(
            token_endpoint = %sanitize_token_endpoint(token_url),
            status = %status,
            body_len = body.len(),
            "OAuth2 token refresh failed; response body redacted"
        );
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
            reason: sanitize_transport_reason("token refresh", &e),
        })
}

/// `OAuth2` client configuration needed to construct an authorization URL and
/// exchange the resulting code for tokens, for a credential key that has no
/// stored credential yet (US4: initial authorization flow).
///
/// This is distinct from [`Credential::OAuth2`](swink_agent::Credential::OAuth2)
/// (which describes an *already-issued* token set): a credential key must be
/// paired with an `AuthorizationConfig` via
/// [`with_authorization_config`](crate::DefaultCredentialResolver::with_authorization_config)
/// before the resolver can build an authorization URL for it. A key with an
/// authorization handler configured but no matching `AuthorizationConfig`
/// behaves as if no handler were configured (FR-011: `NotFound`).
#[derive(Debug, Clone)]
pub struct AuthorizationConfig {
    /// The provider's authorization endpoint (where the user is sent to
    /// grant access), e.g. `https://accounts.google.com/o/oauth2/v2/auth`.
    pub authorization_endpoint: String,
    /// The token endpoint used to exchange the authorization code for
    /// tokens.
    pub token_url: String,
    /// `OAuth2` client identifier.
    pub client_id: String,
    /// `OAuth2` client secret (optional for public clients).
    pub client_secret: Option<String>,
    /// The redirect URI registered with the provider; the authorization
    /// handler is responsible for listening on this address.
    pub redirect_uri: String,
    /// Requested scopes.
    pub scopes: Vec<String>,
}

/// Build the authorization URL for the given config and CSRF `state` token.
///
/// Appends `response_type=code`, `client_id`, `redirect_uri`, `scope`
/// (space-joined, omitted if empty), and `state` as properly percent-encoded
/// query parameters.
pub fn build_authorization_url(
    config: &AuthorizationConfig,
    state: &str,
) -> Result<String, CredentialError> {
    let mut url = reqwest::Url::parse(&config.authorization_endpoint).map_err(|_| {
        CredentialError::AuthorizationFailed {
            key: String::new(),
            reason: "invalid authorization endpoint URL".to_string(),
        }
    })?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("response_type", "code");
        pairs.append_pair("client_id", &config.client_id);
        pairs.append_pair("redirect_uri", &config.redirect_uri);
        if !config.scopes.is_empty() {
            pairs.append_pair("scope", &config.scopes.join(" "));
        }
        pairs.append_pair("state", state);
    }
    Ok(url.to_string())
}

/// Exchange an `OAuth2` authorization code for tokens.
///
/// Sends a POST request with `grant_type=authorization_code` to `token_url`.
/// Returns the parsed token response on success.
///
/// On failure, the returned [`CredentialError::AuthorizationFailed`] contains
/// only a sanitized reason (mirroring [`refresh_token`]'s hygiene): HTTP
/// status plus (if the body is a standard OAuth2 error JSON) the stable
/// `error` code. The raw response body is NEVER included in the surfaced
/// error.
pub async fn exchange_code(
    client: &reqwest::Client,
    token_url: &str,
    code: &str,
    client_id: &str,
    client_secret: Option<&str>,
    redirect_uri: &str,
) -> Result<TokenResponse, CredentialError> {
    debug!(
        token_endpoint = %sanitize_token_endpoint(token_url),
        "exchanging OAuth2 authorization code for tokens"
    );

    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
    ];
    if let Some(secret) = client_secret {
        form.push(("client_secret", secret));
    }

    let response = client
        .post(token_url)
        .form(&form)
        .send()
        .await
        .map_err(|e| CredentialError::AuthorizationFailed {
            key: String::new(),
            reason: sanitize_transport_reason("authorization code exchange", &e),
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        // Raw body stays in debug tracing only; see refresh_token's comment
        // on why this second debug! call (after the .await) is needed.
        debug!(
            token_endpoint = %sanitize_token_endpoint(token_url),
            status = %status,
            body_len = body.len(),
            "OAuth2 authorization code exchange failed; response body redacted"
        );
        let reason = sanitize_code_exchange_reason(status, &body);
        return Err(CredentialError::AuthorizationFailed {
            key: String::new(),
            reason,
        });
    }

    response
        .json::<TokenResponse>()
        .await
        .map_err(|e| CredentialError::AuthorizationFailed {
            key: String::new(),
            reason: sanitize_transport_reason("authorization code exchange", &e),
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
    fn token_response_debug_redacts_tokens() {
        let response = TokenResponse {
            access_token: LEAK_SENTINEL.to_string(),
            refresh_token: Some("REFRESH_LEAK_SENTINEL".to_string()),
            expires_in: Some(3600),
            token_type: Some("Bearer".to_string()),
        };

        let debug = format!("{response:?}");

        assert!(
            !debug.contains(LEAK_SENTINEL),
            "access token leaked: {debug}"
        );
        assert!(
            !debug.contains("REFRESH_LEAK_SENTINEL"),
            "refresh token leaked: {debug}"
        );
        assert!(
            debug.contains("Bearer") && debug.contains("3600"),
            "safe token metadata should remain visible: {debug}"
        );
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

    #[test]
    fn sanitize_token_endpoint_redacts_query_and_path_details() {
        let endpoint = sanitize_token_endpoint(
            "https://user:pass@auth.example.com/token/refresh?client_secret=LEAK_SENTINEL_ABC123",
        );

        assert_eq!(endpoint, "https://auth.example.com/<path>");
        assert!(!endpoint.contains("user"));
        assert!(!endpoint.contains("pass"));
        assert!(!endpoint.contains("refresh"));
        assert!(!endpoint.contains("LEAK_SENTINEL_ABC123"));
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
        let token_url = format!(
            "{}/token?client_secret={LEAK_SENTINEL}&tenant=swink",
            mock_server.uri()
        );

        let logs = SharedLogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .without_time()
            .with_writer(logs.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let err = refresh_token(
            &reqwest::Client::new(),
            &token_url,
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
            !log_output.contains("/token?"),
            "debug log leaked raw token endpoint: {log_output}"
        );
        assert!(
            log_output.contains("127.0.0.1")
                && log_output.contains("<path>")
                && !log_output.contains("client_secret"),
            "debug log should include a sanitized endpoint classification: {log_output}"
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

    #[tokio::test]
    async fn refresh_token_transport_failure_reason_is_sanitized() {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(200))
            .build()
            .unwrap();

        let err = refresh_token(
            &client,
            "http://127.0.0.1:1/token?client_secret=LEAK_SENTINEL_ABC123",
            "refresh-token",
            "client-id",
            Some("client-secret"),
        )
        .await
        .unwrap_err();
        let display = format!("{err}");

        assert!(
            !display.contains("LEAK_SENTINEL_ABC123"),
            "transport error leaked endpoint query details: {display}"
        );
        assert!(
            !display.contains("/token"),
            "transport error leaked endpoint path details: {display}"
        );
        assert!(
            display.contains("transport"),
            "transport error should use a stable sanitized reason: {display}"
        );
    }

    // T060: authorization code exchange

    #[tokio::test]
    async fn exchange_code_success_parses_token_response() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "exchanged-access",
                "refresh_token": "exchanged-refresh",
                "expires_in": 3600,
                "token_type": "Bearer"
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let token_url = format!("{}/token", mock_server.uri());
        let response = exchange_code(
            &reqwest::Client::new(),
            &token_url,
            "auth-code",
            "client-id",
            Some("client-secret"),
            "https://localhost:8080/callback",
        )
        .await
        .unwrap();

        assert_eq!(response.access_token, "exchanged-access");
        assert_eq!(response.refresh_token.as_deref(), Some("exchanged-refresh"));
    }

    #[tokio::test]
    async fn exchange_code_failure_returns_authorization_failed_without_leaking_body() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": format!("code expired {LEAK_SENTINEL}"),
            })))
            .mount(&mock_server)
            .await;

        let token_url = format!("{}/token", mock_server.uri());
        let err = exchange_code(
            &reqwest::Client::new(),
            &token_url,
            "auth-code",
            "client-id",
            Some("client-secret"),
            "https://localhost:8080/callback",
        )
        .await
        .unwrap_err();

        match &err {
            CredentialError::AuthorizationFailed { reason, .. } => {
                assert!(reason.contains("400"));
                assert!(reason.contains("invalid_grant"));
            }
            other => panic!("expected AuthorizationFailed, got {other:?}"),
        }
        let display = format!("{err}");
        assert!(
            !display.contains(LEAK_SENTINEL),
            "error_description leaked into Display: {display}"
        );
    }

    #[tokio::test]
    async fn exchange_code_transport_failure_reason_is_sanitized() {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(200))
            .build()
            .unwrap();

        let err = exchange_code(
            &client,
            "http://127.0.0.1:1/token?client_secret=LEAK_SENTINEL_ABC123",
            "auth-code",
            "client-id",
            Some("client-secret"),
            "https://localhost:8080/callback",
        )
        .await
        .unwrap_err();
        let display = format!("{err}");

        assert!(matches!(err, CredentialError::AuthorizationFailed { .. }));
        assert!(!display.contains("LEAK_SENTINEL_ABC123"));
        assert!(display.contains("transport"));
    }

    // T054 URL-construction check: authorize() must receive a correctly
    // formed authorization URL.
    #[test]
    fn build_authorization_url_includes_expected_query_params() {
        let config = AuthorizationConfig {
            authorization_endpoint: "https://auth.example.com/o/authorize".to_string(),
            token_url: "https://auth.example.com/token".to_string(),
            client_id: "client with spaces".to_string(),
            client_secret: Some("shh".to_string()),
            redirect_uri: "http://localhost:8080/callback".to_string(),
            scopes: vec!["read".to_string(), "write".to_string()],
        };

        let url = build_authorization_url(&config, "csrf-state-123").unwrap();
        let parsed = reqwest::Url::parse(&url).unwrap();
        let pairs: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();

        assert_eq!(pairs.get("response_type").map(String::as_str), Some("code"));
        assert_eq!(
            pairs.get("client_id").map(String::as_str),
            Some("client with spaces")
        );
        assert_eq!(
            pairs.get("redirect_uri").map(String::as_str),
            Some("http://localhost:8080/callback")
        );
        assert_eq!(pairs.get("scope").map(String::as_str), Some("read write"));
        assert_eq!(
            pairs.get("state").map(String::as_str),
            Some("csrf-state-123")
        );
        assert!(
            !url.contains("shh"),
            "client_secret must never appear in the authorization URL: {url}"
        );
    }

    #[test]
    fn build_authorization_url_omits_scope_when_empty() {
        let config = AuthorizationConfig {
            authorization_endpoint: "https://auth.example.com/o/authorize".to_string(),
            token_url: "https://auth.example.com/token".to_string(),
            client_id: "client-1".to_string(),
            client_secret: None,
            redirect_uri: "http://localhost:8080/callback".to_string(),
            scopes: vec![],
        };

        let url = build_authorization_url(&config, "state").unwrap();
        assert!(!url.contains("scope="));
    }

    #[test]
    fn build_authorization_url_rejects_invalid_endpoint() {
        let config = AuthorizationConfig {
            authorization_endpoint: "not a url".to_string(),
            token_url: "https://auth.example.com/token".to_string(),
            client_id: "client-1".to_string(),
            client_secret: None,
            redirect_uri: "http://localhost:8080/callback".to_string(),
            scopes: vec![],
        };

        let err = build_authorization_url(&config, "state").unwrap_err();
        assert!(matches!(err, CredentialError::AuthorizationFailed { .. }));
    }
}
