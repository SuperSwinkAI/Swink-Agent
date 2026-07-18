//! Optional keychain-backed [`CredentialStore`] (feature `keychain`).
//!
//! Persists tool-auth [`Credential`]s in the operating system's native secret
//! store via the [`keyring`] crate:
//!
//! - macOS — Keychain Services
//! - Windows — Credential Manager
//! - Linux/BSD — Secret Service (D-Bus)
//!
//! Unlike [`InMemoryCredentialStore`](crate::InMemoryCredentialStore),
//! credentials written here survive process restarts, so an OAuth2 refresh
//! performed in one run is still usable in the next (FR-021).
//!
//! # Relationship to `tui/src/credentials.rs`
//!
//! The TUI has its own, unrelated keychain module that stores **LLM provider**
//! API keys for the TUI binary. This store is for **tool authentication**
//! secrets resolved through [`CredentialStore`]. The two share the `keyring`
//! crate and nothing else; they use different service names and never read
//! each other's entries.
//!
//! # Blocking I/O
//!
//! Native keychain calls are synchronous and may block for a long time (macOS
//! can prompt the user for keychain access). Every operation is therefore
//! dispatched to [`tokio::task::spawn_blocking`] rather than run inline on an
//! async worker thread. A Tokio runtime must be active.

use std::sync::Arc;

use swink_agent::{Credential, CredentialError, CredentialFuture, CredentialStore};

/// Service name used for keychain entries created by this store.
///
/// Deliberately distinct from the TUI's `swink-agent` service name so the two
/// keychain users never collide.
pub const DEFAULT_SERVICE: &str = "swink-agent-auth";

// ─── KeychainError ──────────────────────────────────────────────────────────

/// Failure from a [`KeychainBackend`] operation.
///
/// Messages are sanitized: they never contain credential values (FR-016).
#[derive(Debug, thiserror::Error)]
pub enum KeychainError {
    /// The backing keychain could not be reached or opened — no default store
    /// on this platform, a locked keyring, or a D-Bus session that is absent
    /// (common in headless CI containers).
    #[error("keychain unavailable: {0}")]
    Unavailable(String),

    /// The keychain was reachable but the read/write/delete failed.
    #[error("keychain access failed: {0}")]
    Access(String),

    /// A credential could not be converted to or from its stored JSON form.
    ///
    /// In practice this means a read found an entry that this crate did not
    /// write (or that was corrupted); serializing a [`Credential`] does not
    /// fail. Either way the payload is never included in the message, since it
    /// may hold secret material.
    #[error("stored keychain entry is not a valid credential")]
    Malformed,
}

impl From<KeychainError> for CredentialError {
    fn from(error: KeychainError) -> Self {
        Self::StoreError(Box::new(error))
    }
}

// ─── KeychainBackend ────────────────────────────────────────────────────────

/// Seam over the native secret store.
///
/// [`KeychainCredentialStore`] owns the serialization and `CredentialStore`
/// plumbing; a backend only moves opaque strings in and out of storage. The
/// production implementation is [`SystemKeychain`]. Tests substitute a fake so
/// they never touch a real OS keychain — CI runners frequently have no
/// unlocked keyring at all.
///
/// Implementations are called from [`tokio::task::spawn_blocking`], so they
/// may block.
pub trait KeychainBackend: Send + Sync + 'static {
    /// Read the secret stored for `service`/`account`, or `None` if absent.
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, KeychainError>;

    /// Write `secret` for `service`/`account`, replacing any existing value.
    fn set(&self, service: &str, account: &str, secret: &str) -> Result<(), KeychainError>;

    /// Remove the entry for `service`/`account`. Deleting an absent entry
    /// MUST succeed (idempotent), matching `CredentialStore::delete`.
    fn delete(&self, service: &str, account: &str) -> Result<(), KeychainError>;
}

// ─── SystemKeychain ─────────────────────────────────────────────────────────

/// [`KeychainBackend`] backed by the real OS keychain via [`keyring`].
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemKeychain;

impl SystemKeychain {
    /// Create a handle to the platform keychain.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    fn entry(service: &str, account: &str) -> Result<keyring::Entry, KeychainError> {
        keyring::Entry::new(service, account).map_err(map_open_error)
    }
}

/// Errors raised while *opening* an entry indicate the store itself is not
/// usable, which callers may want to distinguish from a failed read.
fn map_open_error(error: keyring::Error) -> KeychainError {
    match error {
        keyring::Error::NoDefaultStore
        | keyring::Error::NoStorageAccess(_)
        | keyring::Error::PlatformFailure(_) => KeychainError::Unavailable(error.to_string()),
        other => KeychainError::Access(other.to_string()),
    }
}

impl KeychainBackend for SystemKeychain {
    fn get(&self, service: &str, account: &str) -> Result<Option<String>, KeychainError> {
        match Self::entry(service, account)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            // A missing entry is not an error — `CredentialStore::get` returns
            // `Ok(None)` and lets the resolver decide what that means.
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(map_open_error(error)),
        }
    }

    fn set(&self, service: &str, account: &str, secret: &str) -> Result<(), KeychainError> {
        Self::entry(service, account)?
            .set_password(secret)
            .map_err(map_open_error)
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), KeychainError> {
        match Self::entry(service, account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(map_open_error(error)),
        }
    }
}

// ─── KeychainCredentialStore ────────────────────────────────────────────────

/// A [`CredentialStore`] that persists credentials in the OS keychain.
///
/// Credentials are stored as JSON under the entry `service`/`key`, where `key`
/// is the credential key the tool's `AuthConfig` names. Because
/// [`Credential`] is `serde`-tagged, all three credential types (API key,
/// bearer, OAuth2) round-trip losslessly (SC-007).
///
/// # Example
///
/// ```no_run
/// use std::sync::Arc;
/// use swink_agent::{Credential, CredentialStore};
/// use swink_agent_auth::{DefaultCredentialResolver, KeychainCredentialStore};
///
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let store = KeychainCredentialStore::new();
/// store
///     .set("github", Credential::ApiKey { key: "ghp_example".into() })
///     .await?;
///
/// let resolver = DefaultCredentialResolver::new(Arc::new(store));
/// // Hand `resolver` to `AgentOptions::with_credential_resolver`.
/// # Ok(())
/// # }
/// ```
pub struct KeychainCredentialStore {
    backend: Arc<dyn KeychainBackend>,
    service: String,
}

impl KeychainCredentialStore {
    /// Create a store against the real OS keychain under [`DEFAULT_SERVICE`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_backend(SystemKeychain::new())
    }

    /// Create a store against a custom [`KeychainBackend`].
    #[must_use]
    pub fn with_backend(backend: impl KeychainBackend) -> Self {
        Self {
            backend: Arc::new(backend),
            service: DEFAULT_SERVICE.to_string(),
        }
    }

    /// Builder method overriding the keychain service name (default:
    /// [`DEFAULT_SERVICE`]). Use this to namespace an embedding application's
    /// credentials away from other `swink-agent` processes on the same
    /// machine.
    #[must_use]
    pub fn with_service(mut self, service: impl Into<String>) -> Self {
        self.service = service.into();
        self
    }

    /// Run `op` on the blocking pool with an owned backend handle.
    ///
    /// Keeps the three `CredentialStore` methods free of duplicated
    /// spawn/join/flatten boilerplate.
    fn dispatch<T, F>(&self, op: F) -> CredentialFuture<'_, T>
    where
        T: Send + 'static,
        F: FnOnce(&dyn KeychainBackend, &str) -> Result<T, KeychainError> + Send + 'static,
    {
        let backend = Arc::clone(&self.backend);
        let service = self.service.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || op(backend.as_ref(), &service))
                .await
                // A panic inside the backend surfaces as a store error rather
                // than tearing down the caller's task.
                .map_err(|error| {
                    CredentialError::StoreError(Box::new(KeychainError::Access(format!(
                        "keychain task failed: {error}"
                    ))))
                })?
                .map_err(CredentialError::from)
        })
    }
}

impl Default for KeychainCredentialStore {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for KeychainCredentialStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Only the service name is printed. Entry keys are not enumerated and
        // values are never read here (FR-016).
        f.debug_struct("KeychainCredentialStore")
            .field("service", &self.service)
            .finish()
    }
}

impl CredentialStore for KeychainCredentialStore {
    fn get(&self, key: &str) -> CredentialFuture<'_, Option<Credential>> {
        let key = key.to_string();
        self.dispatch(move |backend, service| {
            let Some(raw) = backend.get(service, &key)? else {
                return Ok(None);
            };
            // The payload is secret, so a parse failure reports only that it
            // was malformed — never the serde error, which can quote input.
            serde_json::from_str(&raw)
                .map(Some)
                .map_err(|_| KeychainError::Malformed)
        })
    }

    fn set(&self, key: &str, credential: Credential) -> CredentialFuture<'_, ()> {
        let key = key.to_string();
        self.dispatch(move |backend, service| {
            let raw = serde_json::to_string(&credential).map_err(|_| KeychainError::Malformed)?;
            backend.set(service, &key, &raw)
        })
    }

    fn delete(&self, key: &str) -> CredentialFuture<'_, ()> {
        let key = key.to_string();
        self.dispatch(move |backend, service| backend.delete(service, &key))
    }
}
