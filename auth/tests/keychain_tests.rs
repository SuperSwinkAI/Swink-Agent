//! Tests for `KeychainCredentialStore` (#1068).
//!
//! Every test drives a `FakeKeychain` rather than the real OS keychain: CI
//! runners have no unlocked keyring (and macOS would prompt), so touching the
//! platform store here would make the suite environment-dependent. The seam is
//! the public `KeychainBackend` trait, so these tests exercise the real
//! serialization, error mapping, and `CredentialStore` plumbing — only the
//! final syscall is substituted.

#![cfg(feature = "keychain")]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use swink_agent::{Credential, CredentialError, CredentialStore};
use swink_agent_auth::{KeychainBackend, KeychainCredentialStore, KeychainError};

// ─── Fakes ──────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct FakeState {
    entries: Mutex<HashMap<(String, String), String>>,
    deletes: AtomicUsize,
}

/// In-process stand-in for the platform keychain.
///
/// Shares its state through an inner `Arc`, so a clone handed to the store
/// still observes what the test asserts on. (The `Arc` is internal rather than
/// wrapping `FakeKeychain` because the orphan rule forbids implementing a
/// foreign trait for `Arc<LocalType>` from an integration test crate.)
#[derive(Debug, Default, Clone)]
struct FakeKeychain {
    state: Arc<FakeState>,
}

impl FakeKeychain {
    fn new() -> Self {
        Self::default()
    }

    fn raw(&self, service: &str, account: &str) -> Option<String> {
        self.state
            .entries
            .lock()
            .unwrap()
            .get(&(service.to_string(), account.to_string()))
            .cloned()
    }

    fn seed_raw(&self, service: &str, account: &str, raw: &str) {
        self.state
            .entries
            .lock()
            .unwrap()
            .insert((service.to_string(), account.to_string()), raw.to_string());
    }

    fn delete_calls(&self) -> usize {
        self.state.deletes.load(Ordering::SeqCst)
    }
}

impl KeychainBackend for FakeKeychain {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, KeychainError> {
        Ok(self.raw(service, account))
    }

    fn set(&self, service: &str, account: &str, secret: &str) -> Result<(), KeychainError> {
        self.state.entries.lock().unwrap().insert(
            (service.to_string(), account.to_string()),
            secret.to_string(),
        );
        Ok(())
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), KeychainError> {
        self.state.deletes.fetch_add(1, Ordering::SeqCst);
        self.state
            .entries
            .lock()
            .unwrap()
            .remove(&(service.to_string(), account.to_string()));
        Ok(())
    }
}

/// Backend that always reports the keychain as unreachable — models a headless
/// Linux box with no D-Bus session, or a locked keyring.
struct UnavailableKeychain;

impl KeychainBackend for UnavailableKeychain {
    fn get(&self, _service: &str, _account: &str) -> Result<Option<String>, KeychainError> {
        Err(KeychainError::Unavailable("no default store".into()))
    }

    fn set(&self, _service: &str, _account: &str, _secret: &str) -> Result<(), KeychainError> {
        Err(KeychainError::Unavailable("no default store".into()))
    }

    fn delete(&self, _service: &str, _account: &str) -> Result<(), KeychainError> {
        Err(KeychainError::Unavailable("no default store".into()))
    }
}

/// Backend whose `get` panics, to prove a panicking backend surfaces as a
/// store error instead of killing the caller's task.
struct PanickingKeychain;

impl KeychainBackend for PanickingKeychain {
    fn get(&self, _service: &str, _account: &str) -> Result<Option<String>, KeychainError> {
        panic!("backend exploded");
    }

    fn set(&self, _service: &str, _account: &str, _secret: &str) -> Result<(), KeychainError> {
        Ok(())
    }

    fn delete(&self, _service: &str, _account: &str) -> Result<(), KeychainError> {
        Ok(())
    }
}

fn oauth2_credential() -> Credential {
    Credential::OAuth2 {
        access_token: "at-123".into(),
        refresh_token: Some("rt-456".into()),
        expires_at: Some(
            chrono::DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        ),
        token_url: "https://example.test/token".into(),
        client_id: "client-1".into(),
        client_secret: Some("cs-789".into()),
        scopes: vec!["calendar.read".into()],
    }
}

// ─── Roundtrip (SC-007) ─────────────────────────────────────────────────────

#[tokio::test]
async fn get_returns_none_for_missing_key() {
    let store = KeychainCredentialStore::with_backend(FakeKeychain::new());
    assert!(store.get("absent").await.unwrap().is_none());
}

#[tokio::test]
async fn api_key_roundtrips() {
    let store = KeychainCredentialStore::with_backend(FakeKeychain::new());
    store
        .set(
            "github",
            Credential::ApiKey {
                key: "ghp_abc123".into(),
            },
        )
        .await
        .unwrap();

    match store.get("github").await.unwrap().unwrap() {
        Credential::ApiKey { key } => assert_eq!(key, "ghp_abc123"),
        other => panic!("expected ApiKey, got {other:?}"),
    }
}

#[tokio::test]
async fn bearer_token_roundtrips_with_expiry() {
    let expires_at = chrono::DateTime::parse_from_rfc3339("2030-06-01T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let store = KeychainCredentialStore::with_backend(FakeKeychain::new());
    store
        .set(
            "api",
            Credential::Bearer {
                token: "tok-1".into(),
                expires_at: Some(expires_at),
            },
        )
        .await
        .unwrap();

    match store.get("api").await.unwrap().unwrap() {
        Credential::Bearer {
            token,
            expires_at: got,
        } => {
            assert_eq!(token, "tok-1");
            assert_eq!(got, Some(expires_at));
        }
        other => panic!("expected Bearer, got {other:?}"),
    }
}

#[tokio::test]
async fn oauth2_roundtrips_all_fields() {
    let store = KeychainCredentialStore::with_backend(FakeKeychain::new());
    store.set("google", oauth2_credential()).await.unwrap();

    match store.get("google").await.unwrap().unwrap() {
        Credential::OAuth2 {
            access_token,
            refresh_token,
            token_url,
            client_id,
            client_secret,
            scopes,
            ..
        } => {
            assert_eq!(access_token, "at-123");
            assert_eq!(refresh_token.as_deref(), Some("rt-456"));
            assert_eq!(token_url, "https://example.test/token");
            assert_eq!(client_id, "client-1");
            assert_eq!(client_secret.as_deref(), Some("cs-789"));
            assert_eq!(scopes, vec!["calendar.read".to_string()]);
        }
        other => panic!("expected OAuth2, got {other:?}"),
    }
}

#[tokio::test]
async fn set_overwrites_existing_credential() {
    let store = KeychainCredentialStore::with_backend(FakeKeychain::new());
    store
        .set("k", Credential::ApiKey { key: "old".into() })
        .await
        .unwrap();
    store
        .set("k", Credential::ApiKey { key: "new".into() })
        .await
        .unwrap();

    match store.get("k").await.unwrap().unwrap() {
        Credential::ApiKey { key } => assert_eq!(key, "new"),
        other => panic!("expected ApiKey, got {other:?}"),
    }
}

#[tokio::test]
async fn delete_removes_credential() {
    let store = KeychainCredentialStore::with_backend(FakeKeychain::new());
    store
        .set("k", Credential::ApiKey { key: "v".into() })
        .await
        .unwrap();
    store.delete("k").await.unwrap();
    assert!(store.get("k").await.unwrap().is_none());
}

#[tokio::test]
async fn delete_is_idempotent_for_missing_key() {
    let backend = FakeKeychain::new();
    let store = KeychainCredentialStore::with_backend(backend.clone());

    store.delete("never-existed").await.unwrap();
    store.delete("never-existed").await.unwrap();

    assert_eq!(backend.delete_calls(), 2);
}

// ─── Service namespacing ────────────────────────────────────────────────────

#[tokio::test]
async fn entries_are_written_under_the_default_service() {
    let backend = FakeKeychain::new();
    let store = KeychainCredentialStore::with_backend(backend.clone());
    store
        .set("k", Credential::ApiKey { key: "v".into() })
        .await
        .unwrap();

    assert!(
        backend
            .raw(swink_agent_auth::DEFAULT_SERVICE, "k")
            .is_some()
    );
}

#[tokio::test]
async fn custom_service_isolates_credentials() {
    let backend = FakeKeychain::new();
    let alpha = KeychainCredentialStore::with_backend(backend.clone()).with_service("alpha");
    let beta = KeychainCredentialStore::with_backend(backend.clone()).with_service("beta");

    alpha
        .set("shared", Credential::ApiKey { key: "a".into() })
        .await
        .unwrap();

    // Same key name, different service — beta must not see alpha's entry.
    assert!(beta.get("shared").await.unwrap().is_none());
    match alpha.get("shared").await.unwrap().unwrap() {
        Credential::ApiKey { key } => assert_eq!(key, "a"),
        other => panic!("expected ApiKey, got {other:?}"),
    }
}

// ─── Error mapping ──────────────────────────────────────────────────────────

#[tokio::test]
async fn unavailable_backend_maps_to_store_error() {
    let store = KeychainCredentialStore::with_backend(UnavailableKeychain);

    let error = store.get("k").await.unwrap_err();
    assert!(matches!(error, CredentialError::StoreError(_)));

    let error = store
        .set("k", Credential::ApiKey { key: "v".into() })
        .await
        .unwrap_err();
    assert!(matches!(error, CredentialError::StoreError(_)));

    let error = store.delete("k").await.unwrap_err();
    assert!(matches!(error, CredentialError::StoreError(_)));
}

#[tokio::test]
async fn malformed_entry_maps_to_store_error() {
    let backend = FakeKeychain::new();
    backend.seed_raw(swink_agent_auth::DEFAULT_SERVICE, "k", "not json at all");
    let store = KeychainCredentialStore::with_backend(backend.clone());

    let error = store.get("k").await.unwrap_err();
    assert!(matches!(error, CredentialError::StoreError(_)));
}

#[tokio::test]
async fn panicking_backend_becomes_store_error_not_a_task_abort() {
    let store = KeychainCredentialStore::with_backend(PanickingKeychain);
    let error = store.get("k").await.unwrap_err();
    assert!(matches!(error, CredentialError::StoreError(_)));
}

// ─── Secret hygiene (FR-016) ────────────────────────────────────────────────

#[tokio::test]
async fn debug_impl_does_not_leak_secrets() {
    let backend = FakeKeychain::new();
    let store = KeychainCredentialStore::with_backend(backend.clone()).with_service("svc");
    store
        .set(
            "k",
            Credential::ApiKey {
                key: "super-secret-value".into(),
            },
        )
        .await
        .unwrap();

    let debug = format!("{store:?}");
    assert!(debug.contains("svc"));
    assert!(!debug.contains("super-secret-value"));
}

#[tokio::test]
async fn malformed_error_message_does_not_echo_stored_payload() {
    let backend = FakeKeychain::new();
    // A corrupt entry whose bytes still contain secret material — the error
    // must describe the failure without quoting any of it.
    backend.seed_raw(
        swink_agent_auth::DEFAULT_SERVICE,
        "k",
        r#"{"type":"ApiKey","key":"super-secret-value""#,
    );
    let store = KeychainCredentialStore::with_backend(backend.clone());

    let error = store.get("k").await.unwrap_err();
    let rendered = format!("{error}{error:?}");
    assert!(!rendered.contains("super-secret-value"));
}

#[test]
fn keychain_error_display_is_sanitized() {
    let error = KeychainError::Malformed;
    assert_eq!(
        error.to_string(),
        "stored keychain entry is not a valid credential"
    );
}

// ─── Trait object / concurrency ─────────────────────────────────────────────

#[tokio::test]
async fn usable_as_a_boxed_credential_store() {
    let store: Arc<dyn CredentialStore> =
        Arc::new(KeychainCredentialStore::with_backend(FakeKeychain::new()));
    store
        .set("k", Credential::ApiKey { key: "v".into() })
        .await
        .unwrap();
    assert!(store.get("k").await.unwrap().is_some());
}

#[tokio::test]
async fn concurrent_access_is_safe() {
    let store: Arc<dyn CredentialStore> =
        Arc::new(KeychainCredentialStore::with_backend(FakeKeychain::new()));

    let mut handles = Vec::new();
    for i in 0..8 {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            let key = format!("key-{i}");
            store
                .set(
                    &key,
                    Credential::ApiKey {
                        key: format!("val-{i}"),
                    },
                )
                .await
                .unwrap();
            assert!(store.get(&key).await.unwrap().is_some());
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }
}
