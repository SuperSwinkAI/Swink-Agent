//! In-memory credential store backed by `Arc<RwLock<HashMap>>`.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use swink_agent::{Credential, CredentialFuture, CredentialStore};

/// Thread-safe in-memory credential store.
///
/// Uses `Arc<RwLock<HashMap>>` for concurrent reads with exclusive writes.
/// Poisoned locks are recovered via [`std::sync::PoisonError::into_inner`].
pub struct InMemoryCredentialStore {
    store: Arc<RwLock<HashMap<String, Credential>>>,
}

impl InMemoryCredentialStore {
    /// Create a new store pre-seeded with the given credentials.
    #[must_use]
    pub fn new(credentials: HashMap<String, Credential>) -> Self {
        Self {
            store: Arc::new(RwLock::new(credentials)),
        }
    }

    /// Create an empty credential store.
    #[must_use]
    pub fn empty() -> Self {
        Self::new(HashMap::new())
    }

    /// Builder method to add a single credential.
    #[must_use]
    pub fn with_credential(self, key: impl Into<String>, credential: Credential) -> Self {
        self.store
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key.into(), credential);
        self
    }
}

impl std::fmt::Debug for InMemoryCredentialStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self
            .store
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        f.debug_struct("InMemoryCredentialStore")
            .field("credential_count", &count)
            .finish()
    }
}

impl CredentialStore for InMemoryCredentialStore {
    fn get(&self, key: &str) -> CredentialFuture<'_, Option<Credential>> {
        let result = self
            .store
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .cloned();
        Box::pin(std::future::ready(Ok(result)))
    }

    fn set(&self, key: &str, credential: Credential) -> CredentialFuture<'_, ()> {
        self.store
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key.to_string(), credential);
        Box::pin(std::future::ready(Ok(())))
    }

    fn delete(&self, key: &str) -> CredentialFuture<'_, ()> {
        self.store
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(key);
        Box::pin(std::future::ready(Ok(())))
    }
}
