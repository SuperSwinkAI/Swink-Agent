#![forbid(unsafe_code)]

/// Shared base for remote HTTP/SSE stream adapters.
///
/// Bundles the three fields that every reqwest-based adapter carries:
/// an endpoint base URL, an API key, and a shared HTTP client.  Using
/// this struct eliminates the repetitive `new()` constructor and
/// redacted [`std::fmt::Debug`] implementation across adapters.
pub struct AdapterBase {
    pub base_url: String,
    pub api_key: String,
    pub client: reqwest::Client,
}

impl AdapterBase {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            client: reqwest::Client::new(),
        }
    }
}

impl std::fmt::Debug for AdapterBase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdapterBase")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}
