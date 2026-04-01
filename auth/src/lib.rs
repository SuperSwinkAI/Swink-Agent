#![forbid(unsafe_code)]
//! Credential management and OAuth2 support for `swink-agent`.
//!
//! This crate provides:
//! - [`InMemoryCredentialStore`] — thread-safe in-memory credential storage
//! - [`DefaultCredentialResolver`] — credential resolution with expiry checking,
//!   OAuth2 refresh, and concurrent request deduplication
//! - OAuth2 token refresh and authorization code exchange helpers

mod in_memory;
pub mod oauth2;
mod resolver;

pub use in_memory::InMemoryCredentialStore;
pub use resolver::DefaultCredentialResolver;
