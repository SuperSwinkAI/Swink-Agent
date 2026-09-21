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
//!
//! # Size limits
//!
//! Windows Credential Manager caps an entry at 2560 bytes of UTF-16, well
//! below an `OAuth2` credential carrying JWTs. When a credential exceeds
//! [`KeychainBackend::max_secret_len`], the store writes it across numbered
//! chunk entries and commits by pointing the primary entry at them, so a
//! failed write never leaves a readable partial credential.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use swink_agent::{
    Credential, CredentialError, CredentialFuture, CredentialStore, SanitizedStoreError,
};

/// Service name used for keychain entries created by this store.
///
/// Deliberately distinct from the TUI's `swink-agent` service name so the two
/// keychain users never collide.
pub const DEFAULT_SERVICE: &str = "swink-agent-auth";

// ─── KeychainError ──────────────────────────────────────────────────────────

/// Failure from a [`KeychainBackend`] operation.
///
/// Messages are sanitized: they never contain credential values (FR-016).
// Deliberately exhaustive: already published exhaustive in 0.13.x; adding
// #[non_exhaustive] now would itself be a semver break, not a hygiene fix.
#[allow(clippy::exhaustive_enums)]
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
        // `KeychainError` messages are sanitized by construction, so the
        // reason may be shown instead of a bare "credential store error".
        Self::StoreError(Box::new(SanitizedStoreError::new(error.to_string())))
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

    /// List every account name this backend holds under `service`.
    ///
    /// Returns `Ok(None)` when the backend cannot enumerate at all, which is
    /// a capability statement rather than a failure — callers that need to
    /// sweep a service (a migration, say) can then plan around it instead of
    /// mistaking an empty store for an unsupported one. `Ok(Some(vec![]))`
    /// means enumeration worked and found nothing.
    ///
    /// The default implementation reports no enumeration support, so existing
    /// backends keep compiling.
    fn list(&self, service: &str) -> Result<Option<Vec<String>>, KeychainError> {
        let _ = service;
        Ok(None)
    }

    /// Largest secret one entry can hold, in UTF-16 code units, or `None` if
    /// the backend has no practical limit (the default).
    ///
    /// [`KeychainCredentialStore`] splits larger credentials across several
    /// entries. The unit is UTF-16 because that is what Windows Credential
    /// Manager measures.
    fn max_secret_len(&self) -> Option<usize> {
        None
    }
}

/// `CRED_MAX_CREDENTIAL_BLOB_SIZE` (2560 bytes), which
/// `windows-native-keyring-store` checks after encoding the secret as UTF-16.
const WINDOWS_MAX_SECRET_UTF16_LEN: usize = 2560 / 2;

// ─── SystemKeychain ─────────────────────────────────────────────────────────

/// [`KeychainBackend`] backed by the real OS keychain via [`keyring`].
// Deliberately exhaustive: already published exhaustive in 0.13.x; adding
// #[non_exhaustive] now would itself be a semver break, not a hygiene fix.
#[allow(clippy::exhaustive_structs)]
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

    fn list(&self, service: &str) -> Result<Option<Vec<String>>, KeychainError> {
        // `keyring::Entry` is the v1 compatibility wrapper and exposes no
        // search, so enumeration goes through `keyring_core` directly. The
        // wrapper is what installs the platform store as the process default,
        // and it only does so on first use — hence the status call first.
        if let Err(error) = keyring::Entry::store_status() {
            return Err(KeychainError::Unavailable(error.to_string()));
        }

        // Search specs are store-specific. Keychain Services and Secret
        // Service filter on `service`; Credential Manager accepts only a
        // `pattern` regex over target names, which encode the service but not
        // in a documented layout — so it over-matches here and the specifier
        // filter below narrows the result on every platform alike.
        let escaped;
        let mut spec = HashMap::new();
        if cfg!(windows) {
            escaped = escape_regex(service);
            spec.insert("pattern", escaped.as_str());
        } else {
            spec.insert("service", service);
        }

        let entries = match keyring_core::Entry::search(&spec) {
            Ok(entries) => entries,
            // Not every store implements search; say so rather than failing.
            Err(keyring_core::Error::NotSupportedByStore(_)) => return Ok(None),
            Err(error) => return Err(map_open_error(error)),
        };
        Ok(Some(
            entries
                .iter()
                .filter_map(keyring_core::Entry::get_specifiers)
                .filter(|(found, _)| found == service)
                .map(|(_, account)| account)
                .collect(),
        ))
    }

    fn max_secret_len(&self) -> Option<usize> {
        cfg!(windows).then_some(WINDOWS_MAX_SECRET_UTF16_LEN)
    }
}

/// Escape regex metacharacters so a service name matches itself literally.
///
/// Windows Credential Manager's search spec is a regex. An unescaped service
/// name is not merely imprecise — one containing `(` is an invalid pattern and
/// fails the search outright.
fn escape_regex(literal: &str) -> String {
    const META: &str = r"\.+*?()|[]{}^$#&-~";
    let mut escaped = String::with_capacity(literal.len());
    for ch in literal.chars() {
        if META.contains(ch) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

// ─── Chunked entries ────────────────────────────────────────────────────────

/// Primary-entry content for a credential split across chunk entries.
///
/// `deny_unknown_fields` keeps a plain credential (which carries `type`) from
/// ever parsing as a manifest.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChunkManifest {
    /// Fresh per write, so a rewrite never overwrites the chunks the current
    /// manifest points at.
    chunk_set: String,
    parts: usize,
}

impl ChunkManifest {
    fn accounts(&self, key: &str) -> impl Iterator<Item = String> {
        let prefix = format!("{key}.chunk.{}", self.chunk_set);
        (0..self.parts).map(move |index| format!("{prefix}.{index}"))
    }
}

/// Whether `account` is one of the chunk entries [`ChunkManifest::accounts`]
/// mints, rather than a credential key a caller chose.
///
/// Matched from the tail (`.chunk.<32 hex>.<index>`) so a credential key that
/// itself contains `.chunk.` is not mistaken for one.
fn is_chunk_account(account: &str) -> bool {
    let Some((head, index)) = account.rsplit_once('.') else {
        return false;
    };
    let Some((head, chunk_set)) = head.rsplit_once('.') else {
        return false;
    };
    let Some((_, marker)) = head.rsplit_once('.') else {
        return false;
    };
    marker == "chunk"
        && !index.is_empty()
        && index.bytes().all(|byte| byte.is_ascii_digit())
        // `Uuid::simple` is exactly 32 hex digits.
        && chunk_set.len() == 32
        && chunk_set.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// What a raw value read out of a keychain service turns out to be.
///
/// Returned by [`classify_stored_entry`], for callers that share a service
/// with this store and must tell its entries apart from their own.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredEntry {
    /// A serialized [`Credential`] written by [`KeychainCredentialStore`].
    Credential,
    /// The primary entry of a chunked credential.
    ///
    /// It holds a manifest pointing at the chunk entries, **not** a secret —
    /// handing this value to a caller as though it were one is a bug.
    ChunkManifest,
    /// Neither: a raw secret, or JSON some other writer owns.
    Foreign,
}

/// Classify a raw keychain value without needing a store or the keychain.
///
/// Sharing one service between this store and another writer is supported but
/// ambiguous: the stored bytes carry no discriminator. Rather than have each
/// caller re-derive the test by parsing [`Credential`] — and so depend on this
/// crate's serde shape by accident — this is the supported predicate.
///
/// Classification is by parse, not by sniffing for a field name, so an
/// ordinary JSON secret is not misread as a credential.
///
/// ```
/// use swink_agent::Credential;
/// use swink_agent_auth::{StoredEntry, classify_stored_entry};
///
/// let envelope = serde_json::to_string(&Credential::ApiKey { key: "k".into() }).unwrap();
/// assert_eq!(classify_stored_entry(&envelope), StoredEntry::Credential);
/// assert_eq!(classify_stored_entry("plain-secret"), StoredEntry::Foreign);
/// ```
#[must_use]
pub fn classify_stored_entry(raw: &str) -> StoredEntry {
    // Manifest first. `deny_unknown_fields` means a `Credential` (which
    // always carries `type`) can never parse as a manifest, but a manifest
    // has no tag to stop it being tried as a credential.
    if serde_json::from_str::<ChunkManifest>(raw).is_ok() {
        StoredEntry::ChunkManifest
    } else if serde_json::from_str::<Credential>(raw).is_ok() {
        StoredEntry::Credential
    } else {
        StoredEntry::Foreign
    }
}

/// The manifest the primary entry holds, if any. Read failures count as "no
/// manifest": the worst case is orphaned, unreadable chunk entries.
fn read_manifest(backend: &dyn KeychainBackend, service: &str, key: &str) -> Option<ChunkManifest> {
    let primary = backend.get(service, key).ok().flatten()?;
    serde_json::from_str(&primary).ok()
}

/// Best-effort removal of chunk entries no manifest points at any more.
fn delete_chunks(
    backend: &dyn KeychainBackend,
    service: &str,
    accounts: impl Iterator<Item = String>,
) {
    for account in accounts {
        if let Err(error) = backend.delete(service, &account) {
            tracing::warn!(%error, "failed to delete stale keychain chunk entry");
        }
    }
}

/// Split `raw` into pieces of at most `limit` UTF-16 code units, never inside
/// a character.
fn split_utf16(raw: &str, limit: usize) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut start, mut units) = (0, 0);
    for (index, ch) in raw.char_indices() {
        if units + ch.len_utf16() > limit {
            parts.push(&raw[start..index]);
            (start, units) = (index, 0);
        }
        units += ch.len_utf16();
    }
    parts.push(&raw[start..]);
    parts
}

fn read_raw(
    backend: &dyn KeychainBackend,
    service: &str,
    key: &str,
) -> Result<Option<String>, KeychainError> {
    let Some(primary) = backend.get(service, key)? else {
        return Ok(None);
    };
    let Ok(manifest) = serde_json::from_str::<ChunkManifest>(&primary) else {
        return Ok(Some(primary));
    };
    let mut raw = String::new();
    for account in manifest.accounts(key) {
        // A missing part means a concurrent rewrite or delete removed it;
        // never hand back a partial credential.
        raw.push_str(
            &backend
                .get(service, &account)?
                .ok_or(KeychainError::Malformed)?,
        );
    }
    Ok(Some(raw))
}

fn write_raw(
    backend: &dyn KeychainBackend,
    service: &str,
    key: &str,
    raw: &str,
) -> Result<(), KeychainError> {
    let Some(limit) = backend.max_secret_len() else {
        return backend.set(service, key, raw);
    };
    let stale = read_manifest(backend, service, key);

    if raw.encode_utf16().count() <= limit {
        backend.set(service, key, raw)?;
    } else {
        let parts = split_utf16(raw, limit);
        let manifest = ChunkManifest {
            chunk_set: uuid::Uuid::new_v4().simple().to_string(),
            parts: parts.len(),
        };
        let manifest_json =
            serde_json::to_string(&manifest).map_err(|_| KeychainError::Malformed)?;
        // Chunks first, manifest last: until the primary entry is replaced,
        // readers still see the previous credential.
        let written = manifest
            .accounts(key)
            .zip(&parts)
            .try_for_each(|(account, part)| backend.set(service, &account, part))
            .and_then(|()| backend.set(service, key, &manifest_json));
        if let Err(error) = written {
            delete_chunks(backend, service, manifest.accounts(key));
            return Err(error);
        }
    }

    if let Some(stale) = stale {
        delete_chunks(backend, service, stale.accounts(key));
    }
    Ok(())
}

fn delete_raw(
    backend: &dyn KeychainBackend,
    service: &str,
    key: &str,
) -> Result<(), KeychainError> {
    let manifest = backend
        .max_secret_len()
        .and_then(|_| read_manifest(backend, service, key));
    // Primary first, so the credential stops being readable before any chunk
    // disappears.
    backend.delete(service, key)?;
    if let Some(manifest) = manifest {
        delete_chunks(backend, service, manifest.accounts(key));
    }
    Ok(())
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

    /// Read the value stored under `key` without parsing it as a
    /// [`Credential`].
    ///
    /// Chunked entries are reassembled, so what comes back is the whole
    /// stored string — never a chunk manifest.
    ///
    /// # When this is the right call
    ///
    /// [`CredentialStore::get`] fails on an entry this crate did not write,
    /// and that failure is *not* safely distinguishable from an unreachable
    /// keychain: [`CredentialError::StoreError`] boxes its payload, and
    /// `CredentialError`'s `Clone` degrades any box it cannot recognize, so a
    /// type test on the error is erased the first time one is cloned.
    /// Treating "keychain is down" as "this is a legacy raw value" would hand
    /// a caller the wrong secret.
    ///
    /// So a caller sharing a service with another writer reads with this and
    /// classifies with [`classify_stored_entry`], where the distinction is
    /// carried by the value rather than by an error: a raw legacy value reads
    /// back fine, and only a genuinely broken keychain returns `Err`.
    ///
    /// # Secrets
    ///
    /// This returns secret material as a plain `String` — that is the point,
    /// but it bypasses the parsing that normally keeps values inside
    /// [`Credential`]. Do not log the result.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialError::StoreError`] if the keychain is unreachable
    /// or a chunk of a chunked entry is missing.
    pub fn get_raw(&self, key: &str) -> CredentialFuture<'_, Option<String>> {
        let key = key.to_string();
        self.dispatch(move |backend, service| read_raw(backend, service, &key))
    }

    /// List the credential keys this store holds under its service.
    ///
    /// Chunk entries are filtered out, so what comes back is the set of keys
    /// [`CredentialStore::get`] accepts — not the raw account names.
    ///
    /// `Ok(None)` means the backing store cannot enumerate (see
    /// [`KeychainBackend::list`]); `Ok(Some(vec![]))` means it can and the
    /// service is empty.
    ///
    /// # Sharing a service
    ///
    /// Keys are listed by name, and names alone cannot say who wrote an
    /// entry. If another writer shares this service, pair each key with
    /// [`classify_stored_entry`] (or with a [`CredentialStore::get`] that
    /// reports [`CredentialError::StoreError`] for entries this crate did not
    /// write) before acting on it.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialError::StoreError`] if the keychain is unreachable
    /// or the enumeration itself fails.
    pub fn list_keys(&self) -> CredentialFuture<'_, Option<Vec<String>>> {
        self.dispatch(|backend, service| {
            Ok(backend.list(service)?.map(|accounts| {
                accounts
                    .into_iter()
                    .filter(|account| !is_chunk_account(account))
                    .collect()
            }))
        })
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
            .finish_non_exhaustive()
    }
}

impl CredentialStore for KeychainCredentialStore {
    fn get(&self, key: &str) -> CredentialFuture<'_, Option<Credential>> {
        let key = key.to_string();
        self.dispatch(move |backend, service| {
            let Some(raw) = read_raw(backend, service, &key)? else {
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
            write_raw(backend, service, &key, &raw)
        })
    }

    fn delete(&self, key: &str) -> CredentialFuture<'_, ()> {
        let key = key.to_string();
        self.dispatch(move |backend, service| delete_raw(backend, service, &key))
    }
}
