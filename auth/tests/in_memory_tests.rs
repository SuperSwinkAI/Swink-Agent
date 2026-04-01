//! Tests for InMemoryCredentialStore (T027-T029).

use std::collections::HashMap;
use std::sync::Arc;

use swink_agent::{Credential, CredentialStore};
use swink_agent_auth::InMemoryCredentialStore;

// T027: new() with pre-seeded credentials
#[tokio::test]
async fn get_returns_seeded_credential() {
    let mut creds = HashMap::new();
    creds.insert(
        "github".to_string(),
        Credential::ApiKey { key: "ghp_abc123".into() },
    );
    let store = InMemoryCredentialStore::new(creds);

    let result = store.get("github").await.unwrap();
    assert!(result.is_some());
    match result.unwrap() {
        Credential::ApiKey { key } => assert_eq!(key, "ghp_abc123"),
        other => panic!("expected ApiKey, got {other:?}"),
    }
}

#[tokio::test]
async fn get_returns_none_for_missing_key() {
    let store = InMemoryCredentialStore::empty();
    let result = store.get("nonexistent").await.unwrap();
    assert!(result.is_none());
}

// T028: set() and delete() roundtrip
#[tokio::test]
async fn set_and_delete_roundtrip() {
    let store = InMemoryCredentialStore::empty();

    // Set
    store
        .set("test-key", Credential::ApiKey { key: "secret".into() })
        .await
        .unwrap();

    // Verify it's stored
    let result = store.get("test-key").await.unwrap();
    assert!(result.is_some());

    // Delete
    store.delete("test-key").await.unwrap();

    // Verify it's gone
    let result = store.get("test-key").await.unwrap();
    assert!(result.is_none());
}

// T028: builder pattern
#[tokio::test]
async fn with_credential_builder() {
    let store = InMemoryCredentialStore::empty()
        .with_credential("key1", Credential::ApiKey { key: "v1".into() })
        .with_credential("key2", Credential::ApiKey { key: "v2".into() });

    assert!(store.get("key1").await.unwrap().is_some());
    assert!(store.get("key2").await.unwrap().is_some());
}

// T029: thread safety — concurrent reads and writes
#[tokio::test]
async fn thread_safety_concurrent_access() {
    let store = Arc::new(
        InMemoryCredentialStore::empty()
            .with_credential("shared", Credential::ApiKey { key: "initial".into() }),
    );

    let mut handles = Vec::new();

    // Spawn 10 concurrent readers
    for _ in 0..10 {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            let result = store.get("shared").await.unwrap();
            assert!(result.is_some());
        }));
    }

    // Spawn 5 concurrent writers
    for i in 0..5 {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            store
                .set(
                    &format!("key-{i}"),
                    Credential::ApiKey { key: format!("val-{i}") },
                )
                .await
                .unwrap();
        }));
    }

    // All tasks should complete without panic
    for handle in handles {
        handle.await.unwrap();
    }

    // Verify writes succeeded
    for i in 0..5 {
        assert!(store.get(&format!("key-{i}")).await.unwrap().is_some());
    }
}

// T067: Debug impl doesn't print credential values
#[test]
fn debug_impl_shows_count_not_values() {
    let store = InMemoryCredentialStore::empty()
        .with_credential("k1", Credential::ApiKey { key: "secret-value".into() })
        .with_credential("k2", Credential::ApiKey { key: "another-secret".into() });

    let debug = format!("{store:?}");
    assert!(debug.contains("credential_count"));
    assert!(debug.contains("2"));
    assert!(!debug.contains("secret-value"));
    assert!(!debug.contains("another-secret"));
}
