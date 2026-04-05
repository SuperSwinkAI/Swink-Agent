use std::pin::Pin;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[cfg(feature = "duckduckgo")]
mod duckduckgo;
#[cfg(feature = "brave")]
mod brave;
#[cfg(feature = "tavily")]
mod tavily;

#[cfg(feature = "duckduckgo")]
pub use duckduckgo::DuckDuckGoProvider;
#[cfg(feature = "brave")]
pub use brave::BraveProvider;
#[cfg(feature = "tavily")]
pub use tavily::TavilyProvider;

/// A single search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Errors from search providers.
#[derive(Debug, Error)]
pub enum SearchError {
    #[error("Network error: {0}")]
    NetworkError(String),
    #[error("Failed to parse response: {0}")]
    ParseError(String),
    #[error("Rate limited by search provider")]
    RateLimited,
    #[error("API key is required but not configured")]
    ApiKeyMissing,
    #[error("Search provider unavailable: {0}")]
    ProviderUnavailable(String),
}

/// Abstraction over different search backends.
pub trait SearchProvider: Send + Sync {
    fn name(&self) -> &str;
    fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Vec<SearchResult>, SearchError>> + Send + '_>>;
}
