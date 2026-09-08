//! Tests for `oauth2`.
#![cfg(test)]

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use super::*;
use reqwest::StatusCode;
use tracing_subscriber::fmt::MakeWriter;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const LEAK_SENTINEL: &str = "LEAK_SENTINEL_ABC123";

/// Build a plain client after installing the ring crypto provider —
/// under `rustls-no-provider` (#1110) a bare `reqwest::Client::new()`
/// panics until a process default provider exists.
fn test_client() -> reqwest::Client {
    crate::ensure_default_crypto_provider();
    reqwest::Client::new()
}

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

/// Pin tracing's global max level to `DEBUG` for the whole test binary.
///
/// The log-capture tests below install only *scoped* (thread-local)
/// subscribers. With no global default, tracing's global `MAX_LEVEL`
/// fast-path — which `debug!` consults before dispatching — flickers as
/// scoped guards are set and dropped across the test harness's worker
/// threads under a shared-process runner (`cargo test`), so a `debug!`
/// can be filtered out before reaching the capture buffer and the
/// assertion fails intermittently.
///
/// Installing a global default at `DEBUG` once keeps `MAX_LEVEL` pinned
/// for the binary's lifetime; the scoped capture subscriber still takes
/// precedence on the test's own thread. The global writer is a sink, so
/// it produces no output. Mirrors `plugins/web`'s `pin_global_info_level`
/// (see #1094).
fn pin_global_debug_level() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let global = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(io::sink)
            .finish();
        // Ignore the error if a global default was already set elsewhere;
        // any global default at DEBUG is enough to pin the level.
        let _ = tracing::subscriber::set_global_default(global);
    });
}

/// Install a capturing subscriber as this thread's default, with the
/// global level pinned so the capture is deterministic under `cargo test`.
fn capture_debug_logs(logs: &SharedLogBuffer) -> tracing::subscriber::DefaultGuard {
    pin_global_debug_level();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .without_time()
        .with_writer(logs.clone())
        .finish();
    tracing::subscriber::set_default(subscriber)
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
    let body =
        format!(r#"{{"trace_id":"abc","internal_message":"db lookup failed {LEAK_SENTINEL}"}}"#);
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
    let _guard = capture_debug_logs(&logs);

    let err = refresh_token(
        &test_client(),
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
    crate::ensure_default_crypto_provider();
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
        &test_client(),
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
        &test_client(),
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
    crate::ensure_default_crypto_provider();
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
    let config = AuthorizationConfig::new(
        "https://auth.example.com/o/authorize",
        "https://auth.example.com/token",
        "client with spaces",
        "http://localhost:8080/callback",
    )
    .with_client_secret("shh")
    .with_scopes(["read", "write"]);

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
    let config = AuthorizationConfig::new(
        "https://auth.example.com/o/authorize",
        "https://auth.example.com/token",
        "client-1",
        "http://localhost:8080/callback",
    );

    let url = build_authorization_url(&config, "state").unwrap();
    assert!(!url.contains("scope="));
}

// ── PKCE (RFC 7636) ──────────────────────────────────────────────────

#[test]
fn pkce_verifier_conforms_to_rfc7636_section_4_1() {
    // 32 octets → 43 base64url chars; the unreserved set is the RFC's
    // only charset requirement. Sample many because the charset claim
    // is over the alphabet, not one draw.
    for _ in 0..256 {
        let verifier = PkceVerifier::generate();
        let s = verifier.secret();
        assert_eq!(s.len(), 43, "verifier length: {s}");
        assert!(
            s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "verifier outside unreserved set: {s}"
        );
    }
    assert_ne!(
        PkceVerifier::generate().secret(),
        PkceVerifier::generate().secret(),
        "two verifiers must not collide"
    );
}

#[test]
fn pkce_challenge_matches_rfc7636_appendix_b_vector() {
    let verifier = PkceVerifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".to_string());
    assert_eq!(
        verifier.challenge(),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

#[test]
fn pkce_verifier_debug_is_redacted() {
    let verifier = PkceVerifier(LEAK_SENTINEL.to_string());
    let rendered = format!("{verifier:?}");
    assert!(
        !rendered.contains(LEAK_SENTINEL),
        "verifier leaked: {rendered}"
    );
    assert!(rendered.contains("REDACTED"));
}

#[test]
fn authorization_url_without_pkce_is_byte_identical_to_public_builder() {
    let config = AuthorizationConfig::new(
        "https://auth.example.com/o/authorize",
        "https://auth.example.com/token",
        "client-1",
        "http://localhost:8080/callback",
    )
    .with_scopes(["read"]);
    assert_eq!(
        build_authorization_url(&config, "state").unwrap(),
        authorization_url(&config, "state", None).unwrap()
    );
    assert!(
        !build_authorization_url(&config, "state")
            .unwrap()
            .contains("code_challenge")
    );
}

#[test]
fn authorization_url_with_pkce_carries_s256_challenge_not_verifier() {
    let config = AuthorizationConfig::new(
        "https://auth.example.com/o/authorize",
        "https://auth.example.com/token",
        "client-1",
        "http://localhost:8080/callback",
    )
    .with_pkce();
    let verifier = PkceVerifier::generate();
    let url = authorization_url(&config, "state", Some(&verifier)).unwrap();
    let parsed = reqwest::Url::parse(&url).unwrap();
    let pairs: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();

    assert_eq!(
        pairs.get("code_challenge").map(String::as_str),
        Some(verifier.challenge().as_str())
    );
    assert_eq!(
        pairs.get("code_challenge_method").map(String::as_str),
        Some("S256")
    );
    assert!(
        !url.contains(verifier.secret()),
        "the verifier must never appear in the authorization URL"
    );
}

#[tokio::test]
async fn exchange_code_with_pkce_sends_code_verifier() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(wiremock::matchers::body_string_contains("code_verifier="))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "tok",
            "token_type": "Bearer"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let verifier = PkceVerifier::generate();
    let response = exchange_code_with_pkce(
        &test_client(),
        &format!("{}/token", server.uri()),
        "code",
        "client-1",
        None,
        "http://localhost:8080/callback",
        Some(&verifier),
    )
    .await
    .unwrap();
    assert_eq!(response.access_token, "tok");

    let body =
        String::from_utf8(server.received_requests().await.unwrap()[0].body.clone()).unwrap();
    assert!(body.contains(&format!("code_verifier={}", verifier.secret())));
}

#[test]
fn build_authorization_url_rejects_invalid_endpoint() {
    let config = AuthorizationConfig::new(
        "not a url",
        "https://auth.example.com/token",
        "client-1",
        "http://localhost:8080/callback",
    );

    let err = build_authorization_url(&config, "state").unwrap_err();
    assert!(matches!(err, CredentialError::AuthorizationFailed { .. }));
}

// ─── Device authorization grant (RFC 8628) ──────────────────────────────

/// Build a sleeper for [`poll_device_token_with_sleep`] that records the
/// durations it is asked to sleep for and returns immediately, making the
/// loop's interval and back-off behavior observable without real time
/// passing. Returns the recording handle alongside the sleeper.
fn recording_sleeper() -> (
    Arc<Mutex<Vec<Duration>>>,
    impl Fn(Duration) -> std::future::Ready<()>,
) {
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let handle = Arc::clone(&recorded);
    let sleeper = move |duration: Duration| {
        handle.lock().unwrap().push(duration);
        std::future::ready(())
    };
    (recorded, sleeper)
}

/// The recorded sleeps, in seconds, in the order the loop performed them.
fn recorded_secs(recorded: &Arc<Mutex<Vec<Duration>>>) -> Vec<u64> {
    recorded
        .lock()
        .unwrap()
        .iter()
        .map(Duration::as_secs)
        .collect()
}

fn device_config(base_url: &str) -> DeviceAuthorizationConfig {
    DeviceAuthorizationConfig::new(
        format!("{base_url}/device/code"),
        format!("{base_url}/token"),
        "client-id",
    )
    .with_client_secret("client-secret")
    .with_scopes(["read"])
}

fn device_response(interval: Option<i64>) -> DeviceAuthorizationResponse {
    DeviceAuthorizationResponse {
        device_code: "device-code-secret".to_string(),
        user_code: "WDJB-MJHT".to_string(),
        verification_uri: "https://auth.example.com/device".to_string(),
        verification_uri_complete: None,
        expires_in: Some(600),
        interval,
    }
}

fn oauth2_error_response(code: &str) -> ResponseTemplate {
    ResponseTemplate::new(400).set_body_json(serde_json::json!({ "error": code }))
}

fn token_success_response() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "access_token": "device-access",
        "refresh_token": "device-refresh",
        "expires_in": 3600,
        "token_type": "Bearer"
    }))
}

#[test]
fn device_authorization_response_debug_redacts_device_code() {
    let response = DeviceAuthorizationResponse {
        device_code: LEAK_SENTINEL.to_string(),
        user_code: "WDJB-MJHT".to_string(),
        verification_uri: "https://auth.example.com/device".to_string(),
        verification_uri_complete: None,
        expires_in: Some(600),
        interval: Some(5),
    };

    let debug = format!("{response:?}");

    assert!(
        !debug.contains(LEAK_SENTINEL),
        "device_code leaked: {debug}"
    );
    assert!(
        debug.contains("WDJB-MJHT") && debug.contains("600"),
        "user-facing prompt fields should remain visible: {debug}"
    );
}

#[test]
fn device_poll_interval_falls_back_when_absent_or_invalid() {
    assert_eq!(
        device_response(Some(3)).poll_interval(),
        Duration::from_secs(3)
    );
    assert_eq!(
        device_response(None).poll_interval(),
        DEFAULT_DEVICE_POLL_INTERVAL
    );
    assert_eq!(
        device_response(Some(0)).poll_interval(),
        DEFAULT_DEVICE_POLL_INTERVAL,
        "a non-positive interval must not busy-poll the provider"
    );
    assert_eq!(
        device_response(Some(-1)).poll_interval(),
        DEFAULT_DEVICE_POLL_INTERVAL
    );
}

#[test]
fn device_lifetime_falls_back_when_expires_in_absent() {
    let mut response = device_response(None);
    response.expires_in = None;
    assert_eq!(response.lifetime(), DEFAULT_DEVICE_CODE_LIFETIME);

    response.expires_in = Some(30);
    assert_eq!(response.lifetime(), Duration::from_secs(30));

    response.expires_in = Some(0);
    assert_eq!(
        response.lifetime(),
        Duration::ZERO,
        "an already-expired code must not be treated as long-lived"
    );
}

#[test]
fn classify_device_poll_response_recognizes_continuable_errors() {
    assert_eq!(
        classify_device_poll_response(
            StatusCode::BAD_REQUEST,
            r#"{"error":"authorization_pending"}"#
        ),
        DevicePollOutcome::Pending
    );
    assert_eq!(
        classify_device_poll_response(StatusCode::BAD_REQUEST, r#"{"error":"slow_down"}"#),
        DevicePollOutcome::SlowDown
    );
}

#[test]
fn classify_device_poll_response_treats_denied_and_expired_as_terminal() {
    let denied =
        classify_device_poll_response(StatusCode::BAD_REQUEST, r#"{"error":"access_denied"}"#);
    assert_eq!(
        denied,
        DevicePollOutcome::Failed(
            "device token request failed: HTTP 400 (access_denied)".to_string()
        )
    );

    let expired =
        classify_device_poll_response(StatusCode::BAD_REQUEST, r#"{"error":"expired_token"}"#);
    assert_eq!(
        expired,
        DevicePollOutcome::Failed(
            "device token request failed: HTTP 400 (expired_token)".to_string()
        )
    );
}

#[test]
fn classify_device_poll_response_redacts_body_in_terminal_reason() {
    // A pending-looking description must not rescue a terminal error, and
    // the description itself must never reach the reason.
    let body = format!(
        r#"{{"error":"access_denied","error_description":"user refused {LEAK_SENTINEL}"}}"#
    );
    let outcome = classify_device_poll_response(StatusCode::BAD_REQUEST, &body);

    match outcome {
        DevicePollOutcome::Failed(reason) => {
            assert!(!reason.contains(LEAK_SENTINEL), "sentinel leaked: {reason}");
            assert!(reason.contains("access_denied"));
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[test]
fn classify_device_poll_response_treats_malformed_body_as_terminal() {
    let body = format!("<html>{LEAK_SENTINEL}</html>");
    let outcome = classify_device_poll_response(StatusCode::INTERNAL_SERVER_ERROR, &body);

    assert_eq!(
        outcome,
        DevicePollOutcome::Failed("device token request failed: HTTP 500".to_string()),
        "an unparseable body must not be mistaken for a continuable error"
    );
}

#[tokio::test]
async fn request_device_code_success_parses_response() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/device/code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "device_code": "dev-123",
            "user_code": "WDJB-MJHT",
            "verification_uri": "https://auth.example.com/device",
            "verification_uri_complete": "https://auth.example.com/device?user_code=WDJB-MJHT",
            "expires_in": 1800,
            "interval": 5
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let config = device_config(&mock_server.uri());
    let response = request_device_code(&test_client(), &config).await.unwrap();

    assert_eq!(response.device_code, "dev-123");
    assert_eq!(response.user_code, "WDJB-MJHT");
    assert_eq!(
        response.verification_uri_complete.as_deref(),
        Some("https://auth.example.com/device?user_code=WDJB-MJHT")
    );
    assert_eq!(response.poll_interval(), Duration::from_secs(5));
    assert_eq!(response.lifetime(), Duration::from_secs(1800));
}

#[tokio::test]
async fn request_device_code_failure_does_not_leak_body() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/device/code"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": "invalid_client",
            "error_description": format!("bad client {LEAK_SENTINEL}"),
        })))
        .mount(&mock_server)
        .await;

    let config = device_config(&mock_server.uri());
    let err = request_device_code(&test_client(), &config)
        .await
        .unwrap_err();

    match &err {
        CredentialError::AuthorizationFailed { reason, .. } => {
            assert_eq!(
                reason,
                "device authorization request failed: HTTP 400 (invalid_client)"
            );
        }
        other => panic!("expected AuthorizationFailed, got {other:?}"),
    }
    assert!(!format!("{err}").contains(LEAK_SENTINEL));
}

#[tokio::test]
async fn request_device_code_transport_failure_reason_is_sanitized() {
    crate::ensure_default_crypto_provider();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(200))
        .build()
        .unwrap();
    let mut config = device_config("http://127.0.0.1:1");
    config.device_authorization_endpoint =
        "http://127.0.0.1:1/device/code?client_secret=LEAK_SENTINEL_ABC123".to_string();

    let err = request_device_code(&client, &config).await.unwrap_err();
    let display = format!("{err}");

    assert!(matches!(err, CredentialError::AuthorizationFailed { .. }));
    assert!(!display.contains("LEAK_SENTINEL_ABC123"));
    assert!(display.contains("transport"));
}

#[tokio::test]
async fn poll_device_token_retries_while_authorization_pending() {
    let mock_server = MockServer::start().await;
    // Two pending polls, then success. `expect(2)` / `expect(1)` assert
    // the loop polled exactly the expected number of times.
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(oauth2_error_response("authorization_pending"))
        .up_to_n_times(2)
        .expect(2)
        .mount(&mock_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(token_success_response())
        .expect(1)
        .mount(&mock_server)
        .await;

    let config = device_config(&mock_server.uri());
    let device = device_response(Some(3));
    let (recorded, sleeper) = recording_sleeper();

    let token = poll_device_token_with_sleep(&test_client(), &config, &device, sleeper)
        .await
        .unwrap();

    assert_eq!(token.access_token, "device-access");
    assert_eq!(token.refresh_token.as_deref(), Some("device-refresh"));
    assert_eq!(
        recorded_secs(&recorded),
        vec![3, 3, 3],
        "authorization_pending must not change the poll interval"
    );
}

#[tokio::test]
async fn poll_device_token_backs_off_on_slow_down() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(oauth2_error_response("slow_down"))
        .up_to_n_times(2)
        .expect(2)
        .mount(&mock_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(token_success_response())
        .expect(1)
        .mount(&mock_server)
        .await;

    let config = device_config(&mock_server.uri());
    let device = device_response(Some(5));
    let (recorded, sleeper) = recording_sleeper();

    let token = poll_device_token_with_sleep(&test_client(), &config, &device, sleeper)
        .await
        .unwrap();

    assert_eq!(token.access_token, "device-access");
    // Each slow_down adds 5s (RFC 8628 §3.5) and the increase persists
    // for every subsequent poll.
    assert_eq!(recorded_secs(&recorded), vec![5, 10, 15]);
}

#[tokio::test]
async fn poll_device_token_interleaves_pending_and_slow_down() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(oauth2_error_response("authorization_pending"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&mock_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(oauth2_error_response("slow_down"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&mock_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(token_success_response())
        .expect(1)
        .mount(&mock_server)
        .await;

    let config = device_config(&mock_server.uri());
    let device = device_response(Some(2));
    let (recorded, sleeper) = recording_sleeper();

    poll_device_token_with_sleep(&test_client(), &config, &device, sleeper)
        .await
        .unwrap();

    // Poll 1 pending (interval unchanged), poll 2 slow_down (bump to 7),
    // poll 3 succeeds.
    assert_eq!(recorded_secs(&recorded), vec![2, 2, 7]);
}

#[tokio::test]
async fn poll_device_token_stops_on_access_denied() {
    let mock_server = MockServer::start().await;
    // `expect(1)` asserts the loop gives up rather than retrying a
    // terminal error.
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(oauth2_error_response("access_denied"))
        .expect(1)
        .mount(&mock_server)
        .await;

    let config = device_config(&mock_server.uri());
    let device = device_response(Some(1));
    let (_recorded, sleeper) = recording_sleeper();

    let err = poll_device_token_with_sleep(&test_client(), &config, &device, sleeper)
        .await
        .unwrap_err();

    match &err {
        CredentialError::AuthorizationFailed { reason, .. } => {
            assert!(
                reason.contains("access_denied"),
                "unexpected reason: {reason}"
            );
        }
        other => panic!("expected AuthorizationFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn poll_device_token_stops_when_provider_reports_expired_token() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(oauth2_error_response("expired_token"))
        .expect(1)
        .mount(&mock_server)
        .await;

    let config = device_config(&mock_server.uri());
    let device = device_response(Some(1));
    let (_recorded, sleeper) = recording_sleeper();

    let err = poll_device_token_with_sleep(&test_client(), &config, &device, sleeper)
        .await
        .unwrap_err();

    match &err {
        CredentialError::AuthorizationFailed { reason, .. } => {
            assert!(
                reason.contains("expired_token"),
                "unexpected reason: {reason}"
            );
        }
        other => panic!("expected AuthorizationFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn poll_device_token_stops_when_device_code_lifetime_elapses() {
    let mock_server = MockServer::start().await;
    // Never mounted to succeed: an already-expired code must fail before
    // any poll is issued.
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(oauth2_error_response("authorization_pending"))
        .expect(0)
        .mount(&mock_server)
        .await;

    let config = device_config(&mock_server.uri());
    let mut device = device_response(Some(1));
    device.expires_in = Some(0);
    let (_recorded, sleeper) = recording_sleeper();

    let err = poll_device_token_with_sleep(&test_client(), &config, &device, sleeper)
        .await
        .unwrap_err();

    match &err {
        CredentialError::AuthorizationFailed { reason, .. } => {
            assert_eq!(reason, "device token request failed: device code expired");
        }
        other => panic!("expected AuthorizationFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn poll_device_token_debug_log_redacts_response_body() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": "access_denied",
            "error_description": format!("refused {LEAK_SENTINEL}"),
        })))
        .mount(&mock_server)
        .await;

    let logs = SharedLogBuffer::default();
    let _guard = capture_debug_logs(&logs);

    let config = device_config(&mock_server.uri());
    let device = device_response(Some(1));
    let (_recorded, sleeper) = recording_sleeper();

    let err = poll_device_token_with_sleep(&test_client(), &config, &device, sleeper)
        .await
        .unwrap_err();
    let log_output = logs.contents();

    assert!(
        !format!("{err}").contains(LEAK_SENTINEL),
        "poll error leaked sentinel: {err}"
    );
    assert!(
        !log_output.contains(LEAK_SENTINEL),
        "debug log leaked response body: {log_output}"
    );
    assert!(
        log_output.contains("body_len") && log_output.contains("response body redacted"),
        "debug log should record redacted body metadata: {log_output}"
    );
}

#[tokio::test]
async fn poll_device_token_transport_failure_reason_is_sanitized() {
    crate::ensure_default_crypto_provider();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(200))
        .build()
        .unwrap();
    let mut config = device_config("http://127.0.0.1:1");
    config.token_url = "http://127.0.0.1:1/token?client_secret=LEAK_SENTINEL_ABC123".to_string();
    let device = device_response(Some(1));
    let (_recorded, sleeper) = recording_sleeper();

    let err = poll_device_token_with_sleep(&client, &config, &device, sleeper)
        .await
        .unwrap_err();
    let display = format!("{err}");

    assert!(matches!(err, CredentialError::AuthorizationFailed { .. }));
    assert!(!display.contains("LEAK_SENTINEL_ABC123"));
    assert!(display.contains("transport"));
}
