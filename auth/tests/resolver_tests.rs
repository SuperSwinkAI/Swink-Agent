//! Tests for DefaultCredentialResolver (T030-T031, T037-T043, T044, T046, T056,
//! T062, T063, T065).

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use swink_agent::{
    AuthorizationHandler, Credential, CredentialError, CredentialFuture, CredentialResolver,
    CredentialStore, ResolvedCredential,
};
use swink_agent_auth::{AuthorizationConfig, DefaultCredentialResolver, InMemoryCredentialStore};

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
    match err {
        CredentialError::Expired { key } => assert_eq!(key, "api"),
        other => panic!("expected Expired, got {other:?}"),
    }
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
    match err {
        CredentialError::Expired { key } => assert_eq!(key, "api"),
        other => panic!("expected Expired, got {other:?}"),
    }
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
    match err {
        CredentialError::Expired { key } => assert_eq!(key, "api"),
        other => panic!("expected Expired, got {other:?}"),
    }
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
    match err {
        CredentialError::Expired { key } => assert_eq!(key, "oauth"),
        other => panic!("expected Expired, got {other:?}"),
    }
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
    match err {
        CredentialError::Expired { key } => assert_eq!(key, "service"),
        other => panic!("expected Expired, got {other:?}"),
    }
}

// ── US4: authorization edge cases ──────────────────────────────────────────

struct UnusedHandler;

impl AuthorizationHandler for UnusedHandler {
    fn authorize(&self, _auth_url: &str, _state: &str) -> CredentialFuture<'_, String> {
        Box::pin(
            async move { unreachable!("handler should not be invoked without a matching config") },
        )
    }
}

// Handler configured, but no AuthorizationConfig registered for the key:
// behaves exactly as if no handler were configured (FR-011).
#[tokio::test]
async fn missing_credential_handler_without_config_returns_not_found() {
    let store: Arc<dyn swink_agent::CredentialStore> = Arc::new(InMemoryCredentialStore::empty());
    let resolver =
        DefaultCredentialResolver::new(store).with_authorization_handler(Arc::new(UnusedHandler));

    let err = resolver.resolve("missing").await.unwrap_err();
    match err {
        CredentialError::NotFound { key } => assert_eq!(key, "missing"),
        other => panic!("expected NotFound, got {other:?}"),
    }
}

/// A `CredentialStore` whose `get()` never completes, used to exercise the
/// resolver-level resolution timeout (T062, FR-014) independent of any
/// timeout applied above the resolver (e.g. dispatch-layer
/// `AgentOptions::with_credential_timeout`).
struct HangingStore;

impl CredentialStore for HangingStore {
    fn get(&self, _key: &str) -> CredentialFuture<'_, Option<Credential>> {
        Box::pin(async move {
            std::future::pending::<()>().await;
            unreachable!("hanging store never completes")
        })
    }

    fn set(&self, _key: &str, _credential: Credential) -> CredentialFuture<'_, ()> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn delete(&self, _key: &str) -> CredentialFuture<'_, ()> {
        Box::pin(std::future::ready(Ok(())))
    }
}

// T062: a resolver-level `with_timeout` bounds the non-interactive
// resolution path even when the store itself never responds.
#[tokio::test]
async fn with_timeout_bounds_store_lookup() {
    let store: Arc<dyn swink_agent::CredentialStore> = Arc::new(HangingStore);
    let resolver = DefaultCredentialResolver::new(store).with_timeout(Duration::from_millis(50));

    let err = resolver.resolve("any-key").await.unwrap_err();
    match err {
        CredentialError::Timeout { key } => assert_eq!(key, "any-key"),
        other => panic!("expected Timeout, got {other:?}"),
    }
}

// T054 negative case: no AuthorizationConfig means the interactive flow
// never starts, so with_authorization_timeout's short window doesn't matter.
#[tokio::test]
async fn with_authorization_config_required_alongside_handler() {
    let store: Arc<dyn swink_agent::CredentialStore> = Arc::new(InMemoryCredentialStore::empty());
    let config = AuthorizationConfig::new(
        "https://accounts.example.com/o/authorize",
        "https://accounts.example.com/token",
        "client",
        "http://localhost:8080/callback",
    );
    // Config registered for a DIFFERENT key than the one being resolved.
    let resolver = DefaultCredentialResolver::new(store)
        .with_authorization_handler(Arc::new(UnusedHandler))
        .with_authorization_config("other-key", config);

    let err = resolver.resolve("missing").await.unwrap_err();
    match err {
        CredentialError::NotFound { key } => assert_eq!(key, "missing"),
        other => panic!("expected NotFound, got {other:?}"),
    }
}
