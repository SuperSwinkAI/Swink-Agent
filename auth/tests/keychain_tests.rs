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
use swink_agent_auth::{
    KeychainBackend, KeychainCredentialStore, KeychainError, StoredEntry, classify_stored_entry,
};

// ─── Fakes ──────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct FakeState {
    entries: Mutex<HashMap<(String, String), String>>,
    deletes: AtomicUsize,
    /// Per-entry cap in UTF-16 code units, enforced like Windows does.
    limit: Option<usize>,
    /// Number of further `set` calls that succeed before every `set` fails.
    sets_before_failure: Mutex<Option<usize>>,
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

    /// A fake enforcing Windows Credential Manager's 2560-byte UTF-16 blob cap.
    fn windows_sized() -> Self {
        Self {
            state: Arc::new(FakeState {
                limit: Some(2560 / 2),
                ..FakeState::default()
            }),
        }
    }

    fn fail_sets_after(&self, successes: usize) {
        *self.state.sets_before_failure.lock().unwrap() = Some(successes);
    }

    fn entry_count(&self) -> usize {
        self.state.entries.lock().unwrap().len()
    }

    fn remove_raw_where(&self, predicate: impl Fn(&str) -> bool) {
        self.state
            .entries
            .lock()
            .unwrap()
            .retain(|(_, account), _| !predicate(account));
    }
}

impl KeychainBackend for FakeKeychain {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, KeychainError> {
        Ok(self.raw(service, account))
    }

    fn set(&self, service: &str, account: &str, secret: &str) -> Result<(), KeychainError> {
        if let Some(remaining) = self.state.sets_before_failure.lock().unwrap().as_mut() {
            if *remaining == 0 {
                return Err(KeychainError::Access("injected write failure".into()));
            }
            *remaining -= 1;
        }
        if self
            .state
            .limit
            .is_some_and(|limit| secret.encode_utf16().count() > limit)
        {
            return Err(KeychainError::Access(
                "secret exceeds platform limit".into(),
            ));
        }
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

    fn list(&self, service: &str) -> Result<Option<Vec<String>>, KeychainError> {
        Ok(Some(
            self.state
                .entries
                .lock()
                .unwrap()
                .keys()
                .filter(|(found, _)| found == service)
                .map(|(_, account)| account.clone())
                .collect(),
        ))
    }

    fn max_secret_len(&self) -> Option<usize> {
        self.state.limit
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

/// An `OAuth2` credential shaped like a Codex sign-in: JWT-sized tokens whose
/// JSON is far past Windows' 1280-UTF-16-unit entry cap. The multi-byte and
/// astral characters make chunk boundaries land near non-ASCII text.
fn large_oauth2_credential() -> Credential {
    Credential::OAuth2 {
        access_token: format!("eyJ{}", "a".repeat(2500)),
        refresh_token: Some(format!("rt-{}", "é😀".repeat(700))),
        expires_at: None,
        token_url: "https://auth.example.test/oauth/token".into(),
        client_id: "app_codex".into(),
        client_secret: None,
        scopes: vec!["openid".into(), "offline_access".into()],
    }
}

fn access_token(credential: &Credential) -> &str {
    match credential {
        Credential::OAuth2 { access_token, .. } => access_token,
        other => panic!("expected OAuth2, got {other:?}"),
    }
}

// ─── Size-limited backends (#1353) ──────────────────────────────────────────

#[tokio::test]
async fn credential_over_windows_blob_limit_roundtrips() {
    let backend = FakeKeychain::windows_sized();
    let store = KeychainCredentialStore::with_backend(backend.clone());
    let credential = large_oauth2_credential();

    store.set("codex", credential.clone()).await.unwrap();

    assert!(backend.entry_count() > 1, "credential was not split");
    let got = store.get("codex").await.unwrap().unwrap();
    assert_eq!(
        serde_json::to_string(&got).unwrap(),
        serde_json::to_string(&credential).unwrap()
    );
}

#[tokio::test]
async fn small_credential_stays_a_single_entry_on_limited_backend() {
    let backend = FakeKeychain::windows_sized();
    let store = KeychainCredentialStore::with_backend(backend.clone());
    store.set("google", oauth2_credential()).await.unwrap();

    assert_eq!(backend.entry_count(), 1);
    assert_eq!(
        access_token(&store.get("google").await.unwrap().unwrap()),
        "at-123"
    );
}

#[tokio::test]
async fn rewriting_a_chunked_credential_removes_stale_chunks() {
    let backend = FakeKeychain::windows_sized();
    let store = KeychainCredentialStore::with_backend(backend.clone());

    store.set("codex", large_oauth2_credential()).await.unwrap();
    store.set("codex", large_oauth2_credential()).await.unwrap();
    let chunked_entries = backend.entry_count();
    store.set("codex", oauth2_credential()).await.unwrap();

    assert!(chunked_entries > 1);
    assert_eq!(backend.entry_count(), 1);
    assert_eq!(
        access_token(&store.get("codex").await.unwrap().unwrap()),
        "at-123"
    );
}

#[tokio::test]
async fn delete_removes_every_chunk() {
    let backend = FakeKeychain::windows_sized();
    let store = KeychainCredentialStore::with_backend(backend.clone());
    store.set("codex", large_oauth2_credential()).await.unwrap();

    store.delete("codex").await.unwrap();

    assert_eq!(backend.entry_count(), 0);
    assert!(store.get("codex").await.unwrap().is_none());
}

#[tokio::test]
async fn failed_chunked_write_keeps_previous_credential_readable() {
    let backend = FakeKeychain::windows_sized();
    let store = KeychainCredentialStore::with_backend(backend.clone());
    store.set("codex", oauth2_credential()).await.unwrap();

    // One chunk lands, the next fails — before the manifest commits.
    backend.fail_sets_after(1);
    let error = store
        .set("codex", large_oauth2_credential())
        .await
        .unwrap_err();

    assert!(
        error.to_string().contains("injected write failure"),
        "{error}"
    );
    assert_eq!(backend.entry_count(), 1, "partial chunks were left behind");
    assert_eq!(
        access_token(&store.get("codex").await.unwrap().unwrap()),
        "at-123"
    );
}

#[tokio::test]
async fn missing_chunk_is_an_error_not_a_partial_credential() {
    let backend = FakeKeychain::windows_sized();
    let store = KeychainCredentialStore::with_backend(backend.clone());
    store.set("codex", large_oauth2_credential()).await.unwrap();

    backend.remove_raw_where(|account| account.ends_with(".0"));

    let error = store.get("codex").await.unwrap_err();
    assert!(matches!(error, CredentialError::StoreError(_)));
}

#[tokio::test]
async fn store_error_display_includes_keychain_reason() {
    let store = KeychainCredentialStore::with_backend(UnavailableKeychain);
    let error = store.get("k").await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "credential store error: keychain unavailable: no default store"
    );
}

#[cfg(windows)]
#[test]
fn system_keychain_reports_windows_blob_limit() {
    assert_eq!(
        swink_agent_auth::SystemKeychain::new().max_secret_len(),
        Some(1280)
    );
}

/// Real Credential Manager round-trip; run manually on a Windows desktop with
/// `cargo test -p swink-agent-auth --features keychain --test integration -- --ignored`.
#[cfg(windows)]
#[tokio::test]
#[ignore = "touches the real Windows Credential Manager"]
async fn real_windows_credential_manager_roundtrips_large_credential() {
    let service = format!("swink-agent-auth-test-{}", std::process::id());
    let store = KeychainCredentialStore::new().with_service(service);
    let credential = large_oauth2_credential();

    store.set("codex", credential.clone()).await.unwrap();
    let got = store.get("codex").await.unwrap().unwrap();
    store.delete("codex").await.unwrap();

    assert_eq!(access_token(&got), access_token(&credential));
    assert!(store.get("codex").await.unwrap().is_none());
}

/// Real OS keychain enumeration (#1358), on whichever platform runs it.
///
/// Unlike the round-trip above this is not Windows-specific: `list` is the
/// one operation with a genuinely different implementation per platform —
/// Keychain Services and Secret Service filter on `service`, Credential
/// Manager over-matches a regex and is narrowed afterwards — so the fake
/// backend cannot cover it, and every platform is worth running it on.
///
/// `cargo test -p swink-agent-auth --features keychain --test integration -- --ignored`
#[tokio::test]
#[ignore = "touches the real OS keychain"]
async fn real_keychain_enumerates_only_its_own_service() {
    let unique = format!("swink-agent-auth-test-{}", std::process::id());
    let store = KeychainCredentialStore::new().with_service(unique.clone());
    let neighbour = KeychainCredentialStore::new().with_service(format!("{unique}-other"));

    // An empty service must enumerate as empty, not as unsupported: that
    // distinction is the whole point of the `Option`.
    assert_eq!(
        store.list_keys().await.unwrap(),
        Some(Vec::new()),
        "search is unsupported on this platform, or the service was not clean"
    );

    store
        .set("alpha", Credential::ApiKey { key: "a".into() })
        .await
        .unwrap();
    store.set("beta", large_oauth2_credential()).await.unwrap();
    // Written under a different service; must never show up below.
    neighbour
        .set("alpha", Credential::ApiKey { key: "n".into() })
        .await
        .unwrap();

    let listed = store.list_keys().await.unwrap().expect("enumerable");
    let mut keys = listed.clone();
    keys.sort();
    assert_eq!(keys, vec!["alpha".to_string(), "beta".to_string()]);
    // Chunk entries are an implementation detail even when the platform
    // makes them (Windows); none may surface as a key.
    assert!(
        !listed.iter().any(|key| key.contains(".chunk.")),
        "chunk entries leaked into list_keys: {listed:?}"
    );

    for key in listed {
        store.delete(&key).await.unwrap();
    }
    neighbour.delete("alpha").await.unwrap();
    assert_eq!(store.list_keys().await.unwrap(), Some(Vec::new()));
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

// ─── Enumeration & classification (#1358) ───────────────────────────────────

/// `list_keys` returns credential keys, not the chunk entries backing them:
/// a caller sweeping a service must see each credential exactly once.
#[tokio::test]
async fn list_keys_reports_each_credential_once_and_hides_chunks() {
    let backend = FakeKeychain::windows_sized();
    let store = KeychainCredentialStore::with_backend(backend.clone());

    // Small enough for one entry; large enough to be split across several.
    store
        .set("small", Credential::ApiKey { key: "k".into() })
        .await
        .unwrap();
    store.set("large", large_oauth2_credential()).await.unwrap();

    // The large credential really did chunk, or this test proves nothing.
    assert!(
        backend.entry_count() > 3,
        "expected chunk entries, got {} entries",
        backend.entry_count()
    );

    let mut keys = store
        .list_keys()
        .await
        .unwrap()
        .expect("backend enumerates");
    keys.sort();
    assert_eq!(keys, vec!["large".to_string(), "small".to_string()]);
}

/// A backend that cannot enumerate must be distinguishable from an empty one,
/// so a migration plans around it instead of concluding there is nothing there.
#[tokio::test]
async fn list_keys_reports_unsupported_separately_from_empty() {
    // `UnavailableKeychain` does not override `list`, so it takes the default.
    let unsupported = KeychainCredentialStore::with_backend(UnavailableKeychain);
    assert_eq!(unsupported.list_keys().await.unwrap(), None);

    let empty = KeychainCredentialStore::with_backend(FakeKeychain::new());
    assert_eq!(empty.list_keys().await.unwrap(), Some(Vec::new()));
}

/// Keys are listed per service, so a store sharing a keychain with another
/// writer never reports entries filed under a different service.
#[tokio::test]
async fn list_keys_is_scoped_to_its_own_service() {
    let backend = FakeKeychain::new();
    backend.seed_raw("other-service", "not-ours", "raw-secret");
    let store = KeychainCredentialStore::with_backend(backend.clone()).with_service("ours");
    store
        .set("mine", Credential::ApiKey { key: "k".into() })
        .await
        .unwrap();

    assert_eq!(
        store.list_keys().await.unwrap(),
        Some(vec!["mine".to_string()])
    );
}

/// A credential key that itself contains `.chunk.` is still a key: the chunk
/// filter matches the minted tail, not the substring.
#[tokio::test]
async fn list_keys_keeps_a_key_that_merely_looks_chunk_like() {
    let backend = FakeKeychain::new();
    let store = KeychainCredentialStore::with_backend(backend.clone());
    for key in ["a.chunk.not-a-uuid.0", "b.chunk.0"] {
        store
            .set(key, Credential::ApiKey { key: "k".into() })
            .await
            .unwrap();
    }

    let mut keys = store.list_keys().await.unwrap().unwrap();
    keys.sort();
    assert_eq!(
        keys,
        vec!["a.chunk.not-a-uuid.0".to_string(), "b.chunk.0".to_string()]
    );
}

/// The three cases a shared service can hold, told apart without the keychain.
#[test]
fn classify_separates_credentials_manifests_and_foreign_values() {
    let envelope = serde_json::to_string(&oauth2_credential()).unwrap();
    assert_eq!(classify_stored_entry(&envelope), StoredEntry::Credential);

    // The shape a chunked credential's primary entry holds. Classified as a
    // manifest rather than a secret, which is the case that otherwise gets
    // handed to a caller as though it were one.
    let manifest = r#"{"chunk_set":"0123456789abcdef0123456789abcdef","parts":2}"#;
    assert_eq!(classify_stored_entry(manifest), StoredEntry::ChunkManifest);

    // Raw values another writer owns — including JSON, which a field-sniffing
    // test would be at risk of misreading.
    for foreign in ["sk-live-abc123", r#"{"token":"abc","scope":"repo"}"#, ""] {
        assert_eq!(
            classify_stored_entry(foreign),
            StoredEntry::Foreign,
            "misclassified {foreign:?}"
        );
    }
}

/// End-to-end of the migration this unblocks: enumerate a shared service,
/// classify each entry, move what this crate wrote, leave the rest alone.
#[tokio::test]
async fn enumerate_then_move_migrates_chunked_credentials_between_services() {
    let backend = FakeKeychain::windows_sized();
    let source = KeychainCredentialStore::with_backend(backend.clone()).with_service("shared");
    let target = KeychainCredentialStore::with_backend(backend.clone()).with_service("moved");

    source
        .set("oauth", large_oauth2_credential())
        .await
        .unwrap();
    // A raw value another writer put in the same service.
    backend.seed_raw("shared", "their-key", "sk-live-abc123");

    let keys = source.list_keys().await.unwrap().unwrap();
    let mut moved = Vec::new();
    for key in keys {
        let raw = backend.raw("shared", &key).expect("listed key exists");
        if classify_stored_entry(&raw) == StoredEntry::Foreign {
            continue;
        }
        let credential = source.get(&key).await.unwrap().expect("readable");
        target.set(&key, credential).await.unwrap();
        source.delete(&key).await.unwrap();
        moved.push(key);
    }

    assert_eq!(moved, vec!["oauth".to_string()]);
    let moved_credential = target.get("oauth").await.unwrap().expect("moved");
    assert_eq!(
        serde_json::to_string(&moved_credential).unwrap(),
        serde_json::to_string(&large_oauth2_credential()).unwrap()
    );
    assert!(source.get("oauth").await.unwrap().is_none());
    // The other writer's entry was neither moved nor removed.
    assert_eq!(
        backend.raw("shared", "their-key").as_deref(),
        Some("sk-live-abc123")
    );
    // And the moved credential left no orphaned chunk entries behind.
    assert!(
        source.list_keys().await.unwrap().unwrap().len() == 1,
        "only the foreign entry should remain under the source service"
    );
}

/// The FR-034 case: a raw value left under a key by another writer reads back
/// intact, while `get` on the same key fails. The distinction Core needs is
/// carried by the value, not by an error that `Clone` would erase.
#[tokio::test]
async fn get_raw_reads_a_foreign_value_that_get_rejects() {
    let backend = FakeKeychain::new();
    backend.seed_raw("swink-agent-auth", "legacy", "sk-live-abc123");
    let store = KeychainCredentialStore::with_backend(backend.clone());

    assert!(
        store.get("legacy").await.is_err(),
        "get must still reject it"
    );
    assert_eq!(
        store.get_raw("legacy").await.unwrap().as_deref(),
        Some("sk-live-abc123")
    );
    assert_eq!(
        classify_stored_entry("sk-live-abc123"),
        StoredEntry::Foreign
    );
}

/// An unreachable keychain stays an error on the raw path too — it must never
/// look like "absent" or like a legacy value.
#[tokio::test]
async fn get_raw_surfaces_an_unreachable_keychain_as_an_error() {
    let store = KeychainCredentialStore::with_backend(UnavailableKeychain);
    assert!(store.get_raw("anything").await.is_err());

    // And a genuinely missing key is `None`, not an error.
    let empty = KeychainCredentialStore::with_backend(FakeKeychain::new());
    assert_eq!(empty.get_raw("absent").await.unwrap(), None);
}

/// A chunked credential reassembles on the raw path: callers never see a
/// manifest, which is the value that would otherwise be mistaken for a secret.
#[tokio::test]
async fn get_raw_reassembles_chunks_rather_than_returning_a_manifest() {
    let backend = FakeKeychain::windows_sized();
    let store = KeychainCredentialStore::with_backend(backend.clone());
    store.set("big", large_oauth2_credential()).await.unwrap();
    assert!(backend.entry_count() > 1, "expected a chunked write");

    let raw = store.get_raw("big").await.unwrap().expect("present");
    assert_eq!(classify_stored_entry(&raw), StoredEntry::Credential);
    assert_eq!(
        raw,
        serde_json::to_string(&large_oauth2_credential()).unwrap()
    );
}
