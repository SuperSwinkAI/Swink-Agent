#![forbid(unsafe_code)]
//! Credential management and OAuth2 support for `swink-agent`.
//!
//! This crate provides:
//! - [`InMemoryCredentialStore`] — thread-safe in-memory credential storage
//! - [`DefaultCredentialResolver`] — credential resolution with expiry checking,
//!   OAuth2 refresh, and concurrent request deduplication
//! - OAuth2 token refresh helpers
//!
//! # Optional features
//!
//! | Feature | Provides |
//! |---|---|
//! | `keychain` | `KeychainCredentialStore` — persists credentials in the OS keychain |
//!
//! No feature is enabled by default; the in-memory store stays the only
//! built-in store unless you opt in. (The table names the type without linking
//! it: under default features it is not compiled, and an intra-doc link to a
//! `cfg`-gated item fails to resolve.)

mod in_memory;

/// Ensure a process-wide default rustls crypto provider is installed.
///
/// The workspace builds reqwest with `rustls-no-provider` (#1110), so a
/// `reqwest::Client` cannot be constructed until a process default
/// [`rustls::crypto::CryptoProvider`] exists. Installs ring; idempotent —
/// an already-installed provider (e.g. a host's aws-lc-rs for FIPS) wins.
pub(crate) fn ensure_default_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
#[cfg(feature = "keychain")]
mod keychain;
pub mod oauth2;
mod resolver;
mod token_source;

pub use in_memory::InMemoryCredentialStore;
#[cfg(feature = "keychain")]
pub use keychain::{
    DEFAULT_SERVICE, KeychainBackend, KeychainCredentialStore, KeychainError, SystemKeychain,
};
pub use oauth2::{AuthorizationConfig, DeviceAuthorizationConfig};
pub use resolver::DefaultCredentialResolver;
pub use token_source::{ExpiringValue, SingleFlightTokenSource};
