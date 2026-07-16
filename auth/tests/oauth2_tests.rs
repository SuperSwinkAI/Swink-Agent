//! Tests for OAuth2 refresh (T045, T047-T049, T064), the interactive
//! authorization flow (T054, T055, T057, T058), and the device authorization
//! grant (RFC 8628, #1071).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use chrono::Utc;
use swink_agent::{
    AuthorizationHandler, Credential, CredentialError, CredentialFuture, CredentialResolver,
    CredentialStore, DeviceCodeHandler, DeviceCodePrompt, ResolvedCredential,
};
use swink_agent_auth::{
    AuthorizationConfig, DefaultCredentialResolver, DeviceAuthorizationConfig,
    InMemoryCredentialStore,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;

/// Helper to create an `Arc<dyn CredentialStore>` from an `InMemoryCredentialStore`.
fn store(s: InMemoryCredentialStore) -> Arc<dyn CredentialStore> {
    Arc::new(s)
}

/// Create an expired OAuth2 credential pointing to the given token URL.
fn expired_oauth2(token_url: &str) -> Credential {
    Credential::OAuth2 {
        access_token: "expired-access".into(),
        refresh_token: Some("my-refresh-token".into()),
        expires_at: Some(Utc::now() - chrono::Duration::hours(1)),
        token_url: token_url.to_string(),
        client_id: "test-client".into(),
        client_secret: Some("test-secret".into()),
        scopes: vec!["read".into()],
    }
}

// T045: Expired OAuth2 with refresh token triggers refresh
#[tokio::test]
async fn expired_oauth2_triggers_refresh() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "new-access-token",
            "refresh_token": "new-refresh-token",
            "expires_in": 3600,
            "token_type": "Bearer"
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let token_url = format!("{}/token", mock_server.uri());
    let store = store(
        InMemoryCredentialStore::empty().with_credential("oauth-key", expired_oauth2(&token_url)),
    );
    let resolver = DefaultCredentialResolver::new(Arc::clone(&store));

    let result = resolver.resolve("oauth-key").await.unwrap();
    match result {
        ResolvedCredential::OAuth2AccessToken(token) => {
            assert_eq!(token, "new-access-token");
        }
        other => panic!("expected OAuth2AccessToken, got {other:?}"),
    }

    // Verify the store was updated
    let updated = store.get("oauth-key").await.unwrap().unwrap();
    match updated {
        Credential::OAuth2 {
            access_token,
            refresh_token,
            ..
        } => {
            assert_eq!(access_token, "new-access-token");
            assert_eq!(refresh_token.as_deref(), Some("new-refresh-token"));
        }
        other => panic!("expected OAuth2, got {other:?}"),
    }
}

#[tokio::test]
async fn expired_oauth2_refresh_preserves_existing_refresh_token_when_not_rotated() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "new-access-token",
            "expires_in": 3600,
            "token_type": "Bearer"
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let token_url = format!("{}/token", mock_server.uri());
    let store = store(
        InMemoryCredentialStore::empty().with_credential("oauth-key", expired_oauth2(&token_url)),
    );
    let resolver = DefaultCredentialResolver::new(Arc::clone(&store));

    let result = resolver.resolve("oauth-key").await.unwrap();
    assert!(matches!(
        result,
        ResolvedCredential::OAuth2AccessToken(token) if token == "new-access-token"
    ));

    let updated = store.get("oauth-key").await.unwrap().unwrap();
    match updated {
        Credential::OAuth2 {
            access_token,
            refresh_token,
            ..
        } => {
            assert_eq!(access_token, "new-access-token");
            assert_eq!(refresh_token.as_deref(), Some("my-refresh-token"));
        }
        other => panic!("expected OAuth2, got {other:?}"),
    }
}

// T047: Refresh fails with HTTP error
#[tokio::test]
async fn refresh_failure_returns_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": "invalid_grant"
        })))
        .mount(&mock_server)
        .await;

    let token_url = format!("{}/token", mock_server.uri());
    let store = Arc::new(
        InMemoryCredentialStore::empty().with_credential("oauth-key", expired_oauth2(&token_url)),
    );
    let resolver = DefaultCredentialResolver::new(store);

    let err = resolver.resolve("oauth-key").await.unwrap_err();
    let err_str = format!("{err}");
    assert!(
        err_str.contains("401") || err_str.contains("refresh") || err_str.contains("failed"),
        "error should indicate refresh failure: {err_str}"
    );
}

// T048: Concurrent resolves result in exactly one HTTP refresh
#[tokio::test]
async fn concurrent_refresh_deduplication() {
    let mock_server = MockServer::start().await;

    // Add a small delay to the response to increase chance of concurrent hits
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({
                    "access_token": "deduped-token",
                    "expires_in": 3600,
                    "token_type": "Bearer"
                }))
                .set_delay(Duration::from_millis(100)),
        )
        .expect(1) // Exactly ONE request
        .mount(&mock_server)
        .await;

    let token_url = format!("{}/token", mock_server.uri());
    let store = store(
        InMemoryCredentialStore::empty().with_credential("shared", expired_oauth2(&token_url)),
    );
    let resolver = Arc::new(DefaultCredentialResolver::new(store));

    // Launch two concurrent resolves
    let r1 = Arc::clone(&resolver);
    let r2 = Arc::clone(&resolver);
    let (res1, res2) = tokio::join!(
        tokio::spawn(async move { r1.resolve("shared").await }),
        tokio::spawn(async move { r2.resolve("shared").await }),
    );

    // Both should succeed
    assert!(res1.unwrap().is_ok());
    assert!(res2.unwrap().is_ok());

    // wiremock will verify exactly 1 request was made (via `.expect(1)`)
}

// T049: Refresh for key A doesn't block key B
#[tokio::test]
async fn refresh_per_key_independence() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "refreshed",
                "expires_in": 3600,
                "token_type": "Bearer"
            })),
        )
        .expect(2) // Two separate keys = two requests
        .mount(&mock_server)
        .await;

    let token_url = format!("{}/token", mock_server.uri());
    let store = store(
        InMemoryCredentialStore::empty()
            .with_credential("key-a", expired_oauth2(&token_url))
            .with_credential("key-b", expired_oauth2(&token_url)),
    );
    let resolver = Arc::new(DefaultCredentialResolver::new(store));

    let r1 = Arc::clone(&resolver);
    let r2 = Arc::clone(&resolver);
    let (res1, res2) = tokio::join!(
        tokio::spawn(async move { r1.resolve("key-a").await }),
        tokio::spawn(async move { r2.resolve("key-b").await }),
    );

    assert!(res1.unwrap().is_ok());
    assert!(res2.unwrap().is_ok());
    // wiremock verifies exactly 2 requests
}

// Issue #613: the raw token-endpoint response body must never leak into the
// surfaced `CredentialError::RefreshFailed` reason / Display output. The
// tests below feed a unique sentinel string in the body and assert the
// sentinel does NOT appear in the public-facing error text.
const LEAK_SENTINEL: &str = "LEAK_SENTINEL_ABC123";

#[tokio::test]
async fn refresh_error_does_not_leak_non_standard_body() {
    let mock_server = MockServer::start().await;

    // Non-standard JSON body with a sensitive-looking sentinel field.
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "trace_id": "t-42",
            "internal_message": format!("raw token dump {LEAK_SENTINEL}"),
        })))
        .mount(&mock_server)
        .await;

    let token_url = format!("{}/token", mock_server.uri());
    let store = Arc::new(
        InMemoryCredentialStore::empty().with_credential("oauth-key", expired_oauth2(&token_url)),
    );
    let resolver = DefaultCredentialResolver::new(store);

    let err = resolver.resolve("oauth-key").await.unwrap_err();
    let display = format!("{err}");
    let debug = format!("{err:?}");

    assert!(
        !display.contains(LEAK_SENTINEL),
        "sentinel leaked into Display: {display}"
    );
    assert!(
        !debug.contains(LEAK_SENTINEL),
        "sentinel leaked into Debug: {debug}"
    );
    assert!(
        display.contains("401"),
        "status missing from Display: {display}"
    );
}

#[tokio::test]
async fn refresh_error_standard_oauth2_surface_includes_error_code_only() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": "invalid_grant",
            "error_description": format!("refresh token expired {LEAK_SENTINEL}"),
        })))
        .mount(&mock_server)
        .await;

    let token_url = format!("{}/token", mock_server.uri());
    let store = Arc::new(
        InMemoryCredentialStore::empty().with_credential("oauth-key", expired_oauth2(&token_url)),
    );
    let resolver = DefaultCredentialResolver::new(store);

    let err = resolver.resolve("oauth-key").await.unwrap_err();
    let display = format!("{err}");

    assert!(
        display.contains("400"),
        "status missing from Display: {display}"
    );
    assert!(
        display.contains("invalid_grant"),
        "standard error code missing from Display: {display}"
    );
    assert!(
        !display.contains(LEAK_SENTINEL),
        "error_description leaked into Display: {display}"
    );
}

#[tokio::test]
async fn refresh_error_malformed_body_degrades_to_status_only() {
    let mock_server = MockServer::start().await;

    let html_body = format!("<html><body>boom {LEAK_SENTINEL}</body></html>");
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(502).set_body_string(html_body))
        .mount(&mock_server)
        .await;

    let token_url = format!("{}/token", mock_server.uri());
    let store = Arc::new(
        InMemoryCredentialStore::empty().with_credential("oauth-key", expired_oauth2(&token_url)),
    );
    let resolver = DefaultCredentialResolver::new(store);

    let err = resolver.resolve("oauth-key").await.unwrap_err();
    let display = format!("{err}");

    assert!(
        !display.contains(LEAK_SENTINEL),
        "sentinel leaked into Display: {display}"
    );
    assert!(
        !display.contains("<html"),
        "raw HTML leaked into Display: {display}"
    );
    assert!(
        display.contains("502"),
        "status missing from Display: {display}"
    );
}

#[tokio::test]
async fn refresh_error_standard_body_ignores_non_standard_fields() {
    // A mostly-standard OAuth2 error body with an extra vendor diagnostic
    // field that serde should ignore. The sentinel lives in the ignored
    // field and must NOT leak.
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": "invalid_grant",
            "debug_info": format!("diagnostic payload {LEAK_SENTINEL}"),
        })))
        .mount(&mock_server)
        .await;

    let token_url = format!("{}/token", mock_server.uri());
    let store = Arc::new(
        InMemoryCredentialStore::empty().with_credential("oauth-key", expired_oauth2(&token_url)),
    );
    let resolver = DefaultCredentialResolver::new(store);

    let err = resolver.resolve("oauth-key").await.unwrap_err();
    let display = format!("{err}");

    assert!(
        !display.contains(LEAK_SENTINEL),
        "non-standard field leaked into Display: {display}"
    );
    assert!(display.contains("invalid_grant"));
    assert!(display.contains("401"));
}

#[tokio::test]
async fn refresh_error_tool_output_path_does_not_leak_body() {
    // Simulates the flow in `src/loop_/tool_dispatch/execute.rs` where a
    // `CredentialError` is converted into tool output via
    // `AgentToolResult::error(format!("{cred_error}"))`. This asserts that
    // the tool-facing string carries no body fragments.
    let mock_server = MockServer::start().await;

    let vendor_body = serde_json::json!({
        "internal_trace": format!("secret {LEAK_SENTINEL} fragment"),
        "request_id": "req-1",
    });
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(500).set_body_json(vendor_body))
        .mount(&mock_server)
        .await;

    let token_url = format!("{}/token", mock_server.uri());
    let store = Arc::new(
        InMemoryCredentialStore::empty().with_credential("oauth-key", expired_oauth2(&token_url)),
    );
    let resolver = DefaultCredentialResolver::new(store);

    let err = resolver.resolve("oauth-key").await.unwrap_err();
    // Mirror execute.rs:203 exactly.
    let tool_output_text = format!("{err}");

    assert!(
        !tool_output_text.contains(LEAK_SENTINEL),
        "sentinel leaked into tool output: {tool_output_text}"
    );
    assert!(
        !tool_output_text.contains("internal_trace"),
        "vendor field name leaked into tool output: {tool_output_text}"
    );
    assert!(tool_output_text.contains("500"));
}

// T064: Pre-provisioned expired OAuth2 auto-refreshes without handler
#[tokio::test]
async fn pre_provisioned_expired_auto_refreshes() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "fresh-token",
            "expires_in": 7200,
            "token_type": "Bearer"
        })))
        .mount(&mock_server)
        .await;

    let token_url = format!("{}/token", mock_server.uri());
    let store =
        store(InMemoryCredentialStore::empty().with_credential("svc", expired_oauth2(&token_url)));
    let resolver = DefaultCredentialResolver::new(store);

    let result = resolver.resolve("svc").await.unwrap();
    match result {
        ResolvedCredential::OAuth2AccessToken(token) => {
            assert_eq!(token, "fresh-token");
        }
        other => panic!("expected OAuth2AccessToken, got {other:?}"),
    }
}

// ── US4: Interactive OAuth2 authorization flow (T054, T055, T057, T058) ────

fn authorization_config(token_url: &str) -> AuthorizationConfig {
    AuthorizationConfig::new(
        "https://accounts.example.com/o/authorize",
        token_url,
        "calendar-client",
        "http://localhost:8080/callback",
    )
    .with_client_secret("calendar-secret")
    .with_scopes(["calendar.readonly"])
}

/// Records the `auth_url` it was invoked with and returns a fixed code.
struct RecordingHandler {
    captured_url: Arc<std::sync::Mutex<Option<String>>>,
    code: String,
}

impl AuthorizationHandler for RecordingHandler {
    fn authorize(&self, auth_url: &str, _state: &str) -> CredentialFuture<'_, String> {
        *self.captured_url.lock().unwrap() = Some(auth_url.to_string());
        let code = self.code.clone();
        Box::pin(async move { Ok(code) })
    }
}

/// Always fails, simulating the user denying access in the browser.
struct DenyingHandler;

impl AuthorizationHandler for DenyingHandler {
    fn authorize(&self, _auth_url: &str, _state: &str) -> CredentialFuture<'_, String> {
        Box::pin(async move {
            Err(CredentialError::AuthorizationFailed {
                key: "google-calendar".into(),
                reason: "user denied access".into(),
            })
        })
    }
}

/// Never completes, simulating a user who never finishes the flow.
struct HangingHandler;

impl AuthorizationHandler for HangingHandler {
    fn authorize(&self, _auth_url: &str, _state: &str) -> CredentialFuture<'_, String> {
        Box::pin(async move {
            std::future::pending::<()>().await;
            unreachable!("hanging handler never completes")
        })
    }
}

/// Counts invocations; used to assert single-flight dedup of concurrent
/// authorization attempts for the same key.
struct CountingHandler {
    calls: Arc<AtomicUsize>,
    code: String,
}

impl AuthorizationHandler for CountingHandler {
    fn authorize(&self, _auth_url: &str, _state: &str) -> CredentialFuture<'_, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let code = self.code.clone();
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(code)
        })
    }
}

// T054: missing credential + handler configured triggers the handler with a
// correctly-formed authorization URL.
#[tokio::test]
async fn missing_credential_with_handler_triggers_authorize_with_correct_url() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "new-access",
            "refresh_token": "new-refresh",
            "expires_in": 3600,
            "token_type": "Bearer"
        })))
        .mount(&mock_server)
        .await;

    let token_url = format!("{}/token", mock_server.uri());
    let captured_url = Arc::new(std::sync::Mutex::new(None));
    let handler = Arc::new(RecordingHandler {
        captured_url: Arc::clone(&captured_url),
        code: "auth-code-123".to_string(),
    });

    let store: Arc<dyn CredentialStore> = Arc::new(InMemoryCredentialStore::empty());
    let resolver = DefaultCredentialResolver::new(store)
        .with_authorization_handler(handler)
        .with_authorization_config("google-calendar", authorization_config(&token_url));

    let result = resolver.resolve("google-calendar").await.unwrap();
    assert!(matches!(
        result,
        ResolvedCredential::OAuth2AccessToken(token) if token == "new-access"
    ));

    let url = captured_url
        .lock()
        .unwrap()
        .clone()
        .expect("handler should have been invoked");
    assert!(url.starts_with("https://accounts.example.com/o/authorize?"));
    assert!(url.contains("response_type=code"));
    assert!(url.contains("client_id=calendar-client"));
    assert!(url.contains("redirect_uri="));
    assert!(url.contains("scope=calendar.readonly"));
    assert!(url.contains("state="));
    assert!(
        !url.contains("calendar-secret"),
        "client_secret must never appear in the authorization URL: {url}"
    );
}

// T055: authorization handler returns a code, the code is exchanged for
// tokens, and the tokens are written to the credential store.
#[tokio::test]
async fn authorization_code_is_exchanged_and_tokens_are_stored() {
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
    let handler = Arc::new(RecordingHandler {
        captured_url: Arc::new(std::sync::Mutex::new(None)),
        code: "auth-code-456".to_string(),
    });

    let store = store(InMemoryCredentialStore::empty());
    let resolver = DefaultCredentialResolver::new(Arc::clone(&store))
        .with_authorization_handler(handler)
        .with_authorization_config("google-calendar", authorization_config(&token_url));

    let result = resolver.resolve("google-calendar").await.unwrap();
    assert!(matches!(
        result,
        ResolvedCredential::OAuth2AccessToken(token) if token == "exchanged-access"
    ));

    let stored = store.get("google-calendar").await.unwrap().unwrap();
    match stored {
        Credential::OAuth2 {
            access_token,
            refresh_token,
            client_id,
            ..
        } => {
            assert_eq!(access_token, "exchanged-access");
            assert_eq!(refresh_token.as_deref(), Some("exchanged-refresh"));
            assert_eq!(client_id, "calendar-client");
        }
        other => panic!("expected OAuth2, got {other:?}"),
    }
}

// T057: authorization handler returns an error -> AuthorizationFailed.
#[tokio::test]
async fn handler_error_returns_authorization_failed() {
    let store: Arc<dyn CredentialStore> = Arc::new(InMemoryCredentialStore::empty());
    let config = authorization_config("https://unused.example.com/token");
    let resolver = DefaultCredentialResolver::new(store)
        .with_authorization_handler(Arc::new(DenyingHandler))
        .with_authorization_config("google-calendar", config);

    let err = resolver.resolve("google-calendar").await.unwrap_err();
    match err {
        CredentialError::AuthorizationFailed { key, reason } => {
            assert_eq!(key, "google-calendar");
            assert_eq!(reason, "user denied access");
        }
        other => panic!("expected AuthorizationFailed, got {other:?}"),
    }
}

// T058: authorization flow exceeds the configured timeout -> AuthorizationTimeout.
#[tokio::test]
async fn authorization_flow_exceeding_timeout_returns_authorization_timeout() {
    let store: Arc<dyn CredentialStore> = Arc::new(InMemoryCredentialStore::empty());
    let config = authorization_config("https://unused.example.com/token");
    let resolver = DefaultCredentialResolver::new(store)
        .with_authorization_handler(Arc::new(HangingHandler))
        .with_authorization_config("google-calendar", config)
        .with_authorization_timeout(Duration::from_millis(50));

    let err = resolver.resolve("google-calendar").await.unwrap_err();
    match err {
        CredentialError::AuthorizationTimeout { key } => assert_eq!(key, "google-calendar"),
        other => panic!("expected AuthorizationTimeout, got {other:?}"),
    }
}

// Concurrent resolves for the same missing key trigger exactly one handler
// invocation (single-flight dedup, mirroring T048 for refresh).
#[tokio::test]
async fn concurrent_resolves_trigger_single_authorization() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "shared-access",
            "expires_in": 3600,
            "token_type": "Bearer"
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let token_url = format!("{}/token", mock_server.uri());
    let calls = Arc::new(AtomicUsize::new(0));
    let handler = Arc::new(CountingHandler {
        calls: Arc::clone(&calls),
        code: "shared-code".to_string(),
    });

    let store: Arc<dyn CredentialStore> = Arc::new(InMemoryCredentialStore::empty());
    let resolver = Arc::new(
        DefaultCredentialResolver::new(store)
            .with_authorization_handler(handler)
            .with_authorization_config("shared-key", authorization_config(&token_url)),
    );

    let r1 = Arc::clone(&resolver);
    let r2 = Arc::clone(&resolver);
    let (res1, res2) = tokio::join!(
        tokio::spawn(async move { r1.resolve("shared-key").await }),
        tokio::spawn(async move { r2.resolve("shared-key").await }),
    );

    assert!(res1.unwrap().is_ok());
    assert!(res2.unwrap().is_ok());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "handler should be invoked exactly once for concurrent resolves"
    );
}

// ─── Device authorization grant (RFC 8628) ──────────────────────────────────

/// Records the prompt it was shown. The device flow's handler is
/// display-only: it returns `()`, not a code.
struct RecordingDeviceHandler {
    captured: Arc<std::sync::Mutex<Option<DeviceCodePrompt>>>,
}

impl DeviceCodeHandler for RecordingDeviceHandler {
    fn present(&self, prompt: &DeviceCodePrompt) -> CredentialFuture<'_, ()> {
        *self.captured.lock().unwrap() = Some(prompt.clone());
        Box::pin(async move { Ok(()) })
    }
}

/// Fails when shown the prompt, simulating a UI that cannot display it.
struct FailingDeviceHandler;

impl DeviceCodeHandler for FailingDeviceHandler {
    fn present(&self, _prompt: &DeviceCodePrompt) -> CredentialFuture<'_, ()> {
        Box::pin(async move {
            Err(CredentialError::AuthorizationFailed {
                key: "device-key".into(),
                reason: "no tty available".into(),
            })
        })
    }
}

fn device_authorization_config(base_url: &str) -> DeviceAuthorizationConfig {
    DeviceAuthorizationConfig::new(
        format!("{base_url}/device/code"),
        format!("{base_url}/token"),
        "device-client",
    )
    .with_scopes(["calendar.readonly"])
}

/// Mount the device authorization endpoint. `interval: 1` keeps the
/// resolver's real polling sleep short; the interval/back-off logic itself is
/// covered by unit tests with an injected sleeper.
async fn mount_device_code_endpoint(mock_server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/device/code"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "device_code": "device-secret-abc",
            "user_code": "WDJB-MJHT",
            "verification_uri": "https://auth.example.com/device",
            "verification_uri_complete": "https://auth.example.com/device?user_code=WDJB-MJHT",
            "expires_in": 600,
            "interval": 1
        })))
        .mount(mock_server)
        .await;
}

#[tokio::test]
async fn missing_credential_with_device_handler_completes_flow_and_stores_credential() {
    let mock_server = MockServer::start().await;
    mount_device_code_endpoint(&mock_server).await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "device-access",
            "refresh_token": "device-refresh",
            "expires_in": 3600,
            "token_type": "Bearer"
        })))
        .mount(&mock_server)
        .await;

    let captured = Arc::new(std::sync::Mutex::new(None));
    let handler = Arc::new(RecordingDeviceHandler {
        captured: Arc::clone(&captured),
    });

    let inner = InMemoryCredentialStore::empty();
    let store_handle: Arc<dyn CredentialStore> = Arc::new(inner);
    let resolver = DefaultCredentialResolver::new(Arc::clone(&store_handle))
        .with_device_code_handler(handler)
        .with_device_authorization_config(
            "device-key",
            device_authorization_config(&mock_server.uri()),
        );

    let resolved = resolver.resolve("device-key").await.unwrap();
    assert!(matches!(
        resolved,
        ResolvedCredential::OAuth2AccessToken(ref t) if t == "device-access"
    ));

    // The user-facing prompt reaches the handler intact.
    let prompt = captured
        .lock()
        .unwrap()
        .clone()
        .expect("handler not invoked");
    assert_eq!(prompt.user_code, "WDJB-MJHT");
    assert_eq!(prompt.verification_uri, "https://auth.example.com/device");
    assert_eq!(
        prompt.verification_uri_complete.as_deref(),
        Some("https://auth.example.com/device?user_code=WDJB-MJHT")
    );
    assert_eq!(prompt.expires_in, Some(600));

    // The issued tokens are persisted so later resolves hit the fast path.
    match store_handle.get("device-key").await.unwrap() {
        Some(Credential::OAuth2 {
            access_token,
            refresh_token,
            client_id,
            ..
        }) => {
            assert_eq!(access_token, "device-access");
            assert_eq!(refresh_token.as_deref(), Some("device-refresh"));
            assert_eq!(client_id, "device-client");
        }
        other => panic!("expected stored OAuth2 credential, got {other:?}"),
    }
}

#[tokio::test]
async fn device_flow_handler_error_aborts_before_polling() {
    let mock_server = MockServer::start().await;
    mount_device_code_endpoint(&mock_server).await;
    // `expect(0)`: a handler that refuses to show the prompt must stop the
    // flow before any token poll is issued.
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&mock_server)
        .await;

    let resolver = DefaultCredentialResolver::new(store(InMemoryCredentialStore::empty()))
        .with_device_code_handler(Arc::new(FailingDeviceHandler))
        .with_device_authorization_config(
            "device-key",
            device_authorization_config(&mock_server.uri()),
        );

    let err = resolver.resolve("device-key").await.unwrap_err();
    match err {
        CredentialError::AuthorizationFailed { reason, .. } => {
            assert_eq!(reason, "no tty available");
        }
        other => panic!("expected AuthorizationFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn device_flow_surfaces_access_denied_from_polling() {
    let mock_server = MockServer::start().await;
    mount_device_code_endpoint(&mock_server).await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": "access_denied"
        })))
        .mount(&mock_server)
        .await;

    let resolver = DefaultCredentialResolver::new(store(InMemoryCredentialStore::empty()))
        .with_device_code_handler(Arc::new(RecordingDeviceHandler {
            captured: Arc::new(std::sync::Mutex::new(None)),
        }))
        .with_device_authorization_config(
            "device-key",
            device_authorization_config(&mock_server.uri()),
        );

    let err = resolver.resolve("device-key").await.unwrap_err();
    match err {
        CredentialError::AuthorizationFailed { key, reason } => {
            assert_eq!(
                key, "device-key",
                "resolver must fill in the credential key"
            );
            assert!(
                reason.contains("access_denied"),
                "unexpected reason: {reason}"
            );
        }
        other => panic!("expected AuthorizationFailed, got {other:?}"),
    }
}

// FR-011 parity: a device-code handler without a matching config for the key
// behaves exactly as if no handler were configured.
#[tokio::test]
async fn device_handler_without_config_for_key_returns_not_found() {
    let mock_server = MockServer::start().await;
    mount_device_code_endpoint(&mock_server).await;

    let resolver = DefaultCredentialResolver::new(store(InMemoryCredentialStore::empty()))
        .with_device_code_handler(Arc::new(RecordingDeviceHandler {
            captured: Arc::new(std::sync::Mutex::new(None)),
        }))
        .with_device_authorization_config(
            "other-key",
            device_authorization_config(&mock_server.uri()),
        );

    let err = resolver.resolve("device-key").await.unwrap_err();
    assert!(
        matches!(err, CredentialError::NotFound { ref key } if key == "device-key"),
        "expected NotFound, got {err:?}"
    );
}

#[tokio::test]
async fn device_config_without_handler_returns_not_found() {
    let mock_server = MockServer::start().await;

    let resolver = DefaultCredentialResolver::new(store(InMemoryCredentialStore::empty()))
        .with_device_authorization_config(
            "device-key",
            device_authorization_config(&mock_server.uri()),
        );

    let err = resolver.resolve("device-key").await.unwrap_err();
    assert!(
        matches!(err, CredentialError::NotFound { ref key } if key == "device-key"),
        "expected NotFound, got {err:?}"
    );
}

// A key configured for both flows keeps its pre-existing authorization-code
// behavior; the device flow must not hijack it.
#[tokio::test]
async fn authorization_code_flow_takes_precedence_over_device_flow() {
    let mock_server = MockServer::start().await;
    // `expect(0)`: the device leg must never be reached for this key.
    Mock::given(method("POST"))
        .and(path("/device/code"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&mock_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "code-flow-access",
            "token_type": "Bearer"
        })))
        .mount(&mock_server)
        .await;

    let token_url = format!("{}/token", mock_server.uri());
    let captured_url = Arc::new(std::sync::Mutex::new(None));
    let device_captured = Arc::new(std::sync::Mutex::new(None));

    let resolver = DefaultCredentialResolver::new(store(InMemoryCredentialStore::empty()))
        .with_authorization_handler(Arc::new(RecordingHandler {
            captured_url: Arc::clone(&captured_url),
            code: "auth-code".to_string(),
        }))
        .with_authorization_config("both-key", authorization_config(&token_url))
        .with_device_code_handler(Arc::new(RecordingDeviceHandler {
            captured: Arc::clone(&device_captured),
        }))
        .with_device_authorization_config(
            "both-key",
            device_authorization_config(&mock_server.uri()),
        );

    let resolved = resolver.resolve("both-key").await.unwrap();

    assert!(matches!(
        resolved,
        ResolvedCredential::OAuth2AccessToken(ref t) if t == "code-flow-access"
    ));
    assert!(
        captured_url.lock().unwrap().is_some(),
        "authorization code handler should have run"
    );
    assert!(
        device_captured.lock().unwrap().is_none(),
        "device handler must not run when the code flow is configured"
    );
}
