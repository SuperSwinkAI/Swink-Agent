use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use swink_agent::AgentEvent;
use swink_agent::plugin::Plugin;
use swink_agent::policy::{PostTurnPolicy, PreDispatchPolicy};
use swink_agent::tool::AgentTool;

use crate::config::{SearchProviderKind, WebPluginConfig, WebPluginConfigBuilder};
use crate::domain::DomainFilter;
use crate::playwright::{PlaywrightBridge, Viewport};
use crate::policy::domain_filter::DomainFilterPolicy;
use crate::policy::rate_limiter::RateLimitPolicy;
use crate::policy::sanitizer::ContentSanitizerPolicy;
use crate::tools::extract::ExtractTool;
use crate::tools::fetch::FetchTool;
use crate::tools::screenshot::ScreenshotTool;
use crate::tools::search::SearchTool;

/// Web browsing plugin for swink-agent.
///
/// Provides tools for fetching web pages and searching the web, along with
/// safety policies for domain filtering, rate limiting, and content sanitization.
pub struct WebPlugin {
    config: WebPluginConfig,
    http_client: reqwest::Client,
    playwright_bridge: Arc<tokio::sync::Mutex<Option<PlaywrightBridge>>>,
    rate_state: Arc<Mutex<VecDeque<Instant>>>,
}

impl WebPlugin {
    /// Create a new `WebPlugin` with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::from_config(WebPluginConfig::default())
    }

    /// Create a builder for custom configuration.
    #[must_use]
    pub fn builder() -> WebPluginConfigBuilder {
        WebPluginConfigBuilder::new()
    }

    /// Create a `WebPlugin` from an explicit configuration.
    #[must_use]
    pub fn from_config(config: WebPluginConfig) -> Self {
        let http_client = reqwest::Client::builder()
            .user_agent(&config.user_agent)
            .redirect(reqwest::redirect::Policy::limited(
                config.max_redirects as usize,
            ))
            .timeout(config.request_timeout)
            .build()
            .expect("Failed to build HTTP client");

        Self {
            config,
            http_client,
            playwright_bridge: Arc::new(tokio::sync::Mutex::new(None)),
            rate_state: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Build the search tool based on the configured search provider.
    fn build_search_tool(&self) -> SearchTool {
        let provider: Arc<dyn crate::search::SearchProvider> =
            match &self.config.search_provider_kind {
                #[cfg(feature = "duckduckgo")]
                SearchProviderKind::DuckDuckGo => Arc::new(crate::search::DuckDuckGoProvider::new(
                    self.http_client.clone(),
                )),
                #[cfg(not(feature = "duckduckgo"))]
                SearchProviderKind::DuckDuckGo => {
                    panic!("DuckDuckGo search provider requires the 'duckduckgo' feature")
                }
                #[cfg(feature = "brave")]
                SearchProviderKind::Brave => {
                    let key = self
                        .config
                        .brave_api_key
                        .clone()
                        .expect("Brave API key required when using Brave search provider");
                    Arc::new(crate::search::BraveProvider::new(
                        key,
                        self.http_client.clone(),
                    ))
                }
                #[cfg(not(feature = "brave"))]
                SearchProviderKind::Brave => {
                    panic!("Brave search provider requires the 'brave' feature")
                }
                #[cfg(feature = "tavily")]
                SearchProviderKind::Tavily => {
                    let key = self
                        .config
                        .tavily_api_key
                        .clone()
                        .expect("Tavily API key required when using Tavily search provider");
                    Arc::new(crate::search::TavilyProvider::new(
                        key,
                        self.http_client.clone(),
                    ))
                }
                #[cfg(not(feature = "tavily"))]
                SearchProviderKind::Tavily => {
                    panic!("Tavily search provider requires the 'tavily' feature")
                }
            };
        SearchTool::new(provider, self.config.max_search_results)
    }
}

impl Default for WebPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for WebPlugin {
    fn name(&self) -> &str {
        "web"
    }

    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        vec![
            Arc::new(FetchTool::new(
                self.http_client.clone(),
                self.config.max_content_length,
                self.config.request_timeout,
            )),
            Arc::new(self.build_search_tool()),
            Arc::new(ScreenshotTool::new(
                self.playwright_bridge.clone(),
                self.config.playwright_path.clone(),
                Viewport {
                    width: self.config.viewport_width,
                    height: self.config.viewport_height,
                },
                self.config.screenshot_timeout,
            )),
            Arc::new(ExtractTool::new(
                self.playwright_bridge.clone(),
                self.config.playwright_path.clone(),
                self.config.screenshot_timeout,
            )),
        ]
    }

    fn pre_dispatch_policies(&self) -> Vec<Arc<dyn PreDispatchPolicy>> {
        let domain_filter = DomainFilter {
            allowlist: self.config.domain_allowlist.clone(),
            denylist: self.config.domain_denylist.clone(),
            block_private_ips: self.config.block_private_ips,
        };
        vec![
            Arc::new(DomainFilterPolicy::new(domain_filter)),
            Arc::new(RateLimitPolicy::new(
                self.rate_state.clone(),
                self.config.rate_limit_rpm,
            )),
        ]
    }

    fn post_turn_policies(&self) -> Vec<Arc<dyn PostTurnPolicy>> {
        if self.config.sanitizer_enabled {
            vec![Arc::new(ContentSanitizerPolicy::new())]
        } else {
            vec![]
        }
    }

    fn on_event(&self, event: &AgentEvent) {
        match event {
            AgentEvent::ToolExecutionStart { name, .. } if name.starts_with("web.") => {
                tracing::info!(tool = %name, "Web tool execution started");
            }
            AgentEvent::ToolExecutionEnd { is_error, .. } if *is_error => {
                tracing::warn!("Web tool execution completed with error");
            }
            _ => {}
        }
    }
}
