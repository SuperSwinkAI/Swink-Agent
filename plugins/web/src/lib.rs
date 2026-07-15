#![forbid(unsafe_code)]
//! Web browsing plugin for swink-agent.
//!
//! Provides tools for fetching web pages, searching the web, capturing screenshots,
//! and extracting structured content. Includes safety policies for domain filtering,
//! rate limiting, and content sanitization.

/// Ensure a process-wide default rustls crypto provider is installed.
///
/// The workspace builds reqwest with `rustls-no-provider` (#1110), so a
/// `reqwest::Client` cannot be constructed until a process default
/// [`rustls::crypto::CryptoProvider`] exists. Installs ring; idempotent —
/// an already-installed provider (e.g. a host's aws-lc-rs for FIPS) wins.
pub(crate) fn ensure_default_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

mod config;
mod content;
mod domain;
mod playwright;
mod plugin;
mod policy;
mod search;
mod tools;

pub use config::{SearchProviderKind, WebPluginConfig, WebPluginConfigBuilder};
pub use content::FetchedContent;
pub use domain::{DomainFilter, DomainFilterError};
pub use playwright::{ExtractedElement, ExtractionPreset, Viewport};
pub use plugin::{WebPlugin, WebPluginError};
pub use search::{SearchError, SearchProvider, SearchResult};
