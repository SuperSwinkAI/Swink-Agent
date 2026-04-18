#![forbid(unsafe_code)]
//! Web browsing plugin for swink-agent.
//!
//! Provides tools for fetching web pages, searching the web, capturing screenshots,
//! and extracting structured content. Includes safety policies for domain filtering,
//! rate limiting, and content sanitization.

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
