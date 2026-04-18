//! Tests for OAuth2 refresh (T045, T047-T049, T064).

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use swink_agent::{Credential, CredentialResolver, CredentialStore, ResolvedCredential};
use swink_agent_auth::{DefaultCredentialResolver, InMemoryCredentialStore};
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
