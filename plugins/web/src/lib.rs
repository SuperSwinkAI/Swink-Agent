//! Web browsing plugin for swink-agent.
//!
//! Provides tools for fetching web pages, searching the web, capturing screenshots,
//! and extracting structured content. Includes safety policies for domain filtering,
//! rate limiting, and content sanitization.

pub mod config;
pub mod content;
pub mod domain;
pub mod playwright;
pub mod plugin;
pub mod policy;
pub mod search;
pub mod tools;

pub use config::{SearchProviderKind, WebPluginConfig, WebPluginConfigBuilder};
pub use content::{
    ContentError, FetchedContent, extract_readable_content, is_html_content_type, truncate_content,
};
pub use domain::{DomainFilter, DomainFilterError};
pub use playwright::{
    ExtractedElement, ExtractionPreset, PlaywrightBridge, PlaywrightError, Viewport,
};
pub use plugin::WebPlugin;
pub use search::{SearchError, SearchProvider, SearchResult};
