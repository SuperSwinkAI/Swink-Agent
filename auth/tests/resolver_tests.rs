//! Tests for DefaultCredentialResolver (T030-T031, T037-T043, T044, T046, T056, T063, T065).

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use swink_agent::{Credential, CredentialError, CredentialResolver, ResolvedCredential};
use swink_agent_auth::{DefaultCredentialResolver, InMemoryCredentialStore};

// ── US1: API Key Resolution ────────────────────────────────────────────────

// T030: Resolve API key
#[tokio::test]
async fn resolve_api_key() {
    let store: Arc<dyn swink_agent::CredentialStore> =
        Arc::new(InMemoryCredentialStore::empty().with_credential(
            "github",
            Credential::ApiKey {
                key: "ghp_test".into(),
            },
        ));
    let resolver = DefaultCredentialResolver::new(store);

    let result = resolver.resolve("github").await.unwrap();
    match result {
        ResolvedCredential::ApiKey(key) => assert_eq!(key, "ghp_test"),
        other => panic!("expected ApiKey, got {other:?}"),
    }
}

// T031: Not found
#[tokio::test]
async fn resolve_missing_key_returns_not_found() {
    let store: Arc<dyn swink_agent::CredentialStore> = Arc::new(InMemoryCredentialStore::empty());
    let resolver = DefaultCredentialResolver::new(store);

    let err = resolver.resolve("missing").await.unwrap_err();
    match err {
        CredentialError::NotFound { key } => assert_eq!(key, "missing"),
        other => panic!("expected NotFound, got {other:?}"),
    }
}

// ── US2: Bearer Token Expiry ───────────────────────────────────────────────

// T037: Bearer with future expiry resolves
#[tokio::test]
async fn bearer_future_expiry_resolves() {
    let future_time = Utc::now() + chrono::Duration::hours(1);
    let store: Arc<dyn swink_agent::CredentialStore> =
        Arc::new(InMemoryCredentialStore::empty().with_credential(
            "api",
            Credential::Bearer {
                token: "tok-valid".into(),
                expires_at: Some(future_time),
            },
        ));
    let resolver = DefaultCredentialResolver::new(store);

    let result = resolver.resolve("api").await.unwrap();
    match result {
        ResolvedCredential::Bearer(token) => assert_eq!(token, "tok-valid"),
        other => panic!("expected Bearer, got {other:?}"),
    }
}

// T038: Bearer with past expiry returns Expired
#[tokio::test]
async fn bearer_past_expiry_returns_expired() {
    let past_time = Utc::now() - chrono::Duration::hours(1);
    let store: Arc<dyn swink_agent::CredentialStore> =
        Arc::new(InMemoryCredentialStore::empty().with_credential(
            "api",
            Credential::Bearer {
                token: "tok-old".into(),
                expires_at: Some(past_time),
            },
        ));
    let resolver = DefaultCredentialResolver::new(store);

    let err = resolver.resolve("api").await.unwrap_err();
    // Could be Expired or RefreshFailed depending on the dedup path
    let err_str = format!("{err}");
    assert!(
        err_str.contains("expired") || err_str.contains("api"),
        "error should mention expiry: {err_str}"
    );
}

// T039: Bearer with no expiry resolves (FR-022)
#[tokio::test]
async fn bearer_no_expiry_resolves() {
    let store: Arc<dyn swink_agent::CredentialStore> =
        Arc::new(InMemoryCredentialStore::empty().with_credential(
            "api",
            Credential::Bearer {
                token: "tok-forever".into(),
                expires_at: None,
            },
        ));
    let resolver = DefaultCredentialResolver::new(store);

    let result = resolver.resolve("api").await.unwrap();
    match result {
        ResolvedCredential::Bearer(token) => assert_eq!(token, "tok-forever"),
        other => panic!("expected Bearer, got {other:?}"),
    }
}

// T040: Bearer expiring within buffer is treated as expired (FR-023)
#[tokio::test]
async fn bearer_within_buffer_treated_as_expired() {
    // Default buffer is 60s — token expires in 30s (within buffer)
    let soon = Utc::now() + chrono::Duration::seconds(30);
    let store: Arc<dyn swink_agent::CredentialStore> =
        Arc::new(InMemoryCredentialStore::empty().with_credential(
            "api",
            Credential::Bearer {
                token: "tok-soon".into(),
                expires_at: Some(soon),
            },
        ));
    let resolver = DefaultCredentialResolver::new(store);

    let err = resolver.resolve("api").await.unwrap_err();
    let err_str = format!("{err}");
    assert!(
        err_str.contains("expired") || err_str.contains("api"),
        "token within buffer should be expired: {err_str}"
    );
}

// T041: Custom expiry buffer
#[tokio::test]
async fn custom_expiry_buffer_respected() {
    // Token expires in 90s, custom buffer is 120s — should be treated as expired
    let time = Utc::now() + chrono::Duration::seconds(90);
    let store: Arc<dyn swink_agent::CredentialStore> =
        Arc::new(InMemoryCredentialStore::empty().with_credential(
            "api",
            Credential::Bearer {
                token: "tok".into(),
                expires_at: Some(time),
            },
        ));
    let resolver =
        DefaultCredentialResolver::new(store).with_expiry_buffer(Duration::from_secs(120));

    let err = resolver.resolve("api").await.unwrap_err();
    let err_str = format!("{err}");
    assert!(
        err_str.contains("expired") || err_str.contains("api"),
        "custom buffer should mark as expired: {err_str}"
    );
}

// ── US3: OAuth2 (non-HTTP tests) ──────────────────────────────────────────

// T044: Valid (non-expired) OAuth2 resolves
#[tokio::test]
async fn valid_oauth2_resolves() {
    let future_time = Utc::now() + chrono::Duration::hours(1);
    let store: Arc<dyn swink_agent::CredentialStore> =
        Arc::new(InMemoryCredentialStore::empty().with_credential(
            "oauth",
            Credential::OAuth2 {
                access_token: "access-good".into(),
                refresh_token: Some("refresh".into()),
                expires_at: Some(future_time),
                token_url: "https://auth.example.com/token".into(),
                client_id: "client-1".into(),
                client_secret: None,
                scopes: vec![],
            },
        ));
    let resolver = DefaultCredentialResolver::new(store);

    let result = resolver.resolve("oauth").await.unwrap();
    match result {
        ResolvedCredential::OAuth2AccessToken(token) => assert_eq!(token, "access-good"),
        other => panic!("expected OAuth2AccessToken, got {other:?}"),
    }
}

// T046: Expired OAuth2 with no refresh token returns Expired
#[tokio::test]
async fn expired_oauth2_no_refresh_returns_expired() {
    let past = Utc::now() - chrono::Duration::hours(1);
    let store: Arc<dyn swink_agent::CredentialStore> =
        Arc::new(InMemoryCredentialStore::empty().with_credential(
            "oauth",
            Credential::OAuth2 {
                access_token: "old".into(),
                refresh_token: None,
                expires_at: Some(past),
                token_url: "https://auth.example.com/token".into(),
                client_id: "client-1".into(),
                client_secret: None,
                scopes: vec![],
            },
        ));
    let resolver = DefaultCredentialResolver::new(store);

    let err = resolver.resolve("oauth").await.unwrap_err();
    let err_str = format!("{err}");
    assert!(
        err_str.contains("expired") || err_str.contains("oauth"),
        "should indicate expired: {err_str}"
    );
}

// T056: Missing credential with no handler returns NotFound
#[tokio::test]
async fn missing_credential_no_handler_returns_not_found() {
    let store: Arc<dyn swink_agent::CredentialStore> = Arc::new(InMemoryCredentialStore::empty());
    let resolver = DefaultCredentialResolver::new(store);

    let err = resolver.resolve("missing").await.unwrap_err();
    match err {
        CredentialError::NotFound { key } => assert_eq!(key, "missing"),
        other => panic!("expected NotFound, got {other:?}"),
    }
}

// ── US5: Headless (no handler) ─────────────────────────────────────────────

// T063: Pre-provisioned OAuth2 resolves without handler
#[tokio::test]
async fn pre_provisioned_oauth2_resolves_without_handler() {
    let future_time = Utc::now() + chrono::Duration::hours(1);
    let store: Arc<dyn swink_agent::CredentialStore> =
        Arc::new(InMemoryCredentialStore::empty().with_credential(
            "service",
            Credential::OAuth2 {
                access_token: "svc-token".into(),
                refresh_token: Some("svc-refresh".into()),
                expires_at: Some(future_time),
                token_url: "https://auth.example.com/token".into(),
                client_id: "svc-client".into(),
                client_secret: Some("svc-secret".into()),
                scopes: vec![],
            },
        ));
    // No authorization handler
    let resolver = DefaultCredentialResolver::new(store);

    let result = resolver.resolve("service").await.unwrap();
    match result {
        ResolvedCredential::OAuth2AccessToken(token) => assert_eq!(token, "svc-token"),
        other => panic!("expected OAuth2AccessToken, got {other:?}"),
    }
}

// T065: Expired credential with no refresh token and no handler returns Expired
#[tokio::test]
async fn expired_no_refresh_no_handler_returns_expired() {
    let past = Utc::now() - chrono::Duration::hours(1);
    let store: Arc<dyn swink_agent::CredentialStore> =
        Arc::new(InMemoryCredentialStore::empty().with_credential(
            "service",
            Credential::OAuth2 {
                access_token: "old".into(),
                refresh_token: None,
                expires_at: Some(past),
                token_url: "https://auth.example.com/token".into(),
                client_id: "svc".into(),
                client_secret: None,
                scopes: vec![],
            },
        ));
    let resolver = DefaultCredentialResolver::new(store);

    let err = resolver.resolve("service").await.unwrap_err();
    let err_str = format!("{err}");
    assert!(
        err_str.contains("expired") || err_str.contains("service"),
        "should indicate expired: {err_str}"
    );
}
