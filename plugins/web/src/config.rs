use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Which search backend to use.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum SearchProviderKind {
    #[default]
    DuckDuckGo,
    Brave,
    Tavily,
}

/// Configuration for the web plugin.
#[derive(Debug, Clone)]
pub struct WebPluginConfig {
    pub search_provider_kind: SearchProviderKind,
    pub brave_api_key: Option<String>,
    pub tavily_api_key: Option<String>,
    pub domain_allowlist: Vec<String>,
    pub domain_denylist: Vec<String>,
    pub block_private_ips: bool,
    pub rate_limit_rpm: u32,
    pub max_content_length: usize,
    pub max_redirects: u32,
    pub max_search_results: usize,
    pub playwright_path: Option<PathBuf>,
    pub screenshot_timeout: Duration,
    pub request_timeout: Duration,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub sanitizer_enabled: bool,
    pub user_agent: String,
}

impl Default for WebPluginConfig {
    fn default() -> Self {
        Self {
            search_provider_kind: SearchProviderKind::default(),
            brave_api_key: None,
            tavily_api_key: None,
            domain_allowlist: Vec::new(),
            domain_denylist: Vec::new(),
            block_private_ips: true,
            rate_limit_rpm: 30,
            max_content_length: 50_000,
            max_redirects: 10,
            max_search_results: 10,
            playwright_path: None,
            screenshot_timeout: Duration::from_secs(15),
            request_timeout: Duration::from_secs(30),
            viewport_width: 1280,
            viewport_height: 720,
            sanitizer_enabled: true,
            user_agent: String::from("SwinkAgent/0.5"),
        }
    }
}

/// Builder for `WebPluginConfig`.
#[derive(Debug, Default)]
pub struct WebPluginConfigBuilder {
    config: WebPluginConfig,
}

impl WebPluginConfigBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_search_provider(mut self, kind: SearchProviderKind) -> Self {
        self.config.search_provider_kind = kind;
        self
    }

    #[must_use]
    pub fn with_brave_api_key(mut self, key: impl Into<String>) -> Self {
        self.config.brave_api_key = Some(key.into());
        self
    }

    #[must_use]
    pub fn with_tavily_api_key(mut self, key: impl Into<String>) -> Self {
        self.config.tavily_api_key = Some(key.into());
        self
    }

    #[must_use]
    pub fn with_domain_allowlist(mut self, domains: Vec<String>) -> Self {
        self.config.domain_allowlist = domains;
        self
    }

    #[must_use]
    pub fn with_domain_denylist(mut self, domains: Vec<String>) -> Self {
        self.config.domain_denylist = domains;
        self
    }

    #[must_use]
    pub fn with_block_private_ips(mut self, block: bool) -> Self {
        self.config.block_private_ips = block;
        self
    }

    #[must_use]
    pub fn with_rate_limit_rpm(mut self, rpm: u32) -> Self {
        self.config.rate_limit_rpm = rpm;
        self
    }

    #[must_use]
    pub fn with_max_content_length(mut self, length: usize) -> Self {
        self.config.max_content_length = length;
        self
    }

    #[must_use]
    pub fn with_max_redirects(mut self, max: u32) -> Self {
        self.config.max_redirects = max;
        self
    }

    #[must_use]
    pub fn with_max_search_results(mut self, max: usize) -> Self {
        self.config.max_search_results = max;
        self
    }

    #[must_use]
    pub fn with_playwright_path(mut self, path: PathBuf) -> Self {
        self.config.playwright_path = Some(path);
        self
    }

    #[must_use]
    pub fn with_screenshot_timeout(mut self, timeout: Duration) -> Self {
        self.config.screenshot_timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.config.request_timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_viewport(mut self, width: u32, height: u32) -> Self {
        self.config.viewport_width = width;
        self.config.viewport_height = height;
        self
    }

    #[must_use]
    pub fn with_sanitizer_enabled(mut self, enabled: bool) -> Self {
        self.config.sanitizer_enabled = enabled;
        self
    }

    #[must_use]
    pub fn with_user_agent(mut self, ua: impl Into<String>) -> Self {
        self.config.user_agent = ua.into();
        self
    }

    #[must_use]
    pub fn build(self) -> WebPluginConfig {
        self.config
    }
}
