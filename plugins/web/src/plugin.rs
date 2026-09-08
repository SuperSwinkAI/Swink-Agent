use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use thiserror::Error;

use swink_agent::AgentEvent;
use swink_agent::{AgentTool, Plugin, PostTurnPolicy, PreDispatchPolicy};

use crate::config::{SearchProviderKind, WebPluginConfig, WebPluginConfigBuilder};
use crate::domain::DomainFilter;
use crate::playwright::{PlaywrightBridge, Viewport};
use crate::policy::{ContentSanitizerPolicy, DomainFilterPolicy, RateLimitPolicy};
use crate::search::SearchProvider;
use crate::tools::{ExtractTool, FetchTool, ScreenshotTool, SearchTool};

/// Errors returned when constructing a [`WebPlugin`].
///
/// These replace the previous panics on recoverable misconfiguration so that
/// hosts embedding the plugin can surface a diagnostic instead of aborting.
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum WebPluginError {
    /// The underlying `reqwest` HTTP client failed to build (e.g. invalid
    /// TLS backend configuration).
    #[error("failed to build HTTP client: {0}")]
    HttpClient(#[from] reqwest::Error),

    /// The configured search provider is not available because the
    /// corresponding cargo feature is disabled at compile time.
    #[error(
        "search provider `{provider}` requires the `{feature}` feature to be enabled at compile time"
    )]
    SearchProviderFeatureDisabled {
        provider: &'static str,
        feature: &'static str,
    },

    /// The configured search provider requires an API key that was not
    /// supplied on the configuration.
    #[error("search provider `{provider}` requires an API key but none was configured")]
    MissingApiKey { provider: &'static str },
}

/// Web browsing plugin for swink-agent.
///
/// Provides tools for fetching web pages and searching the web, along with
/// safety policies for domain filtering, rate limiting, and content sanitization.
pub struct WebPlugin {
    config: WebPluginConfig,
    search_provider: Arc<dyn SearchProvider>,
    playwright_bridge: Arc<tokio::sync::Mutex<Option<PlaywrightBridge>>>,
    rate_state: Arc<Mutex<VecDeque<Instant>>>,
}

impl WebPlugin {
    /// Create a new `WebPlugin` with default configuration.
    ///
    /// Returns an error if the default configuration cannot be satisfied
    /// (for example, if the default search provider's feature flag is
    /// disabled at compile time).
    pub fn new() -> Result<Self, WebPluginError> {
        Self::from_config(WebPluginConfig::default())
    }

    /// Create a builder for custom configuration.
    #[must_use]
    pub fn builder() -> WebPluginConfigBuilder {
        WebPluginConfigBuilder::new()
    }

    /// Create a `WebPlugin` from an explicit configuration.
    ///
    /// Returns an error if the HTTP client cannot be built or if the
    /// configured search provider is unavailable (missing feature flag or
    /// missing API key).
    pub fn from_config(config: WebPluginConfig) -> Result<Self, WebPluginError> {
        crate::ensure_default_crypto_provider();
        let http_client = reqwest::Client::builder()
            .user_agent(&config.user_agent)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(config.request_timeout)
            .build()?;

        let search_provider = build_search_provider(&config, &http_client)?;

        Ok(Self {
            config,
            search_provider,
            playwright_bridge: Arc::new(tokio::sync::Mutex::new(None)),
            rate_state: Arc::new(Mutex::new(VecDeque::new())),
        })
    }
}

#[allow(unused_variables)]
fn build_search_provider(
    config: &WebPluginConfig,
    http_client: &reqwest::Client,
) -> Result<Arc<dyn SearchProvider>, WebPluginError> {
    match &config.search_provider_kind {
        #[cfg(feature = "duckduckgo")]
        SearchProviderKind::DuckDuckGo => Ok(Arc::new(crate::search::DuckDuckGoProvider::new(
            http_client.clone(),
        ))),
        #[cfg(not(feature = "duckduckgo"))]
        SearchProviderKind::DuckDuckGo => Err(WebPluginError::SearchProviderFeatureDisabled {
            provider: "duckduckgo",
            feature: "duckduckgo",
        }),
        #[cfg(feature = "brave")]
        SearchProviderKind::Brave => {
            let key = config
                .brave_api_key
                .clone()
                .ok_or(WebPluginError::MissingApiKey { provider: "brave" })?;
            Ok(Arc::new(crate::search::BraveProvider::new(
                key,
                http_client.clone(),
            )))
        }
        #[cfg(not(feature = "brave"))]
        SearchProviderKind::Brave => Err(WebPluginError::SearchProviderFeatureDisabled {
            provider: "brave",
            feature: "brave",
        }),
        #[cfg(feature = "tavily")]
        SearchProviderKind::Tavily => {
            let key = config
                .tavily_api_key
                .clone()
                .ok_or(WebPluginError::MissingApiKey { provider: "tavily" })?;
            Ok(Arc::new(crate::search::TavilyProvider::new(
                key,
                http_client.clone(),
            )))
        }
        #[cfg(not(feature = "tavily"))]
        SearchProviderKind::Tavily => Err(WebPluginError::SearchProviderFeatureDisabled {
            provider: "tavily",
            feature: "tavily",
        }),
    }
}

impl Plugin for WebPlugin {
    fn name(&self) -> &str {
        "web"
    }

    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        let domain_filter = DomainFilter {
            allowlist: self.config.domain_allowlist.clone(),
            denylist: self.config.domain_denylist.clone(),
            block_private_ips: self.config.block_private_ips,
        };

        vec![
            Arc::new(
                FetchTool::new(self.config.max_content_length, self.config.request_timeout)
                    .with_user_agent(self.config.user_agent.clone())
                    .with_domain_filter(domain_filter.clone(), self.config.max_redirects)
                    .with_sanitizer_enabled(self.config.sanitizer_enabled),
            ),
            Arc::new(
                SearchTool::new(self.search_provider.clone(), self.config.max_search_results)
                    .with_sanitizer_enabled(self.config.sanitizer_enabled),
            ),
            Arc::new(
                ScreenshotTool::new(
                    self.playwright_bridge.clone(),
                    self.config.playwright_path.clone(),
                    Viewport {
                        width: self.config.viewport_width,
                        height: self.config.viewport_height,
                    },
                    self.config.screenshot_timeout,
                )
                .with_domain_filter(domain_filter.clone()),
            ),
            Arc::new(
                ExtractTool::new(
                    self.playwright_bridge.clone(),
                    self.config.playwright_path.clone(),
                    self.config.screenshot_timeout,
                )
                .with_domain_filter(domain_filter)
                .with_sanitizer_enabled(self.config.sanitizer_enabled),
            ),
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
        match classify_web_event(event) {
            WebEventClass::Start(name) => {
                tracing::info!(tool = %name, "Web tool execution started");
            }
            WebEventClass::Error(name) => {
                tracing::warn!(tool = %name, "Web tool execution completed with error");
            }
            WebEventClass::Ignored => {}
        }
    }
}

/// Classification of an [`AgentEvent`] from the web plugin's perspective.
///
/// Extracted from [`WebPlugin::on_event`] so the namespace-gating logic can be
/// exercised directly by unit tests without depending on a tracing subscriber.
#[derive(Debug, Clone, PartialEq, Eq)]
enum WebEventClass<'a> {
    /// A `web_*` tool has started executing.
    Start(&'a str),
    /// A `web_*` tool has failed.
    Error(&'a str),
    /// Not a web-namespaced tool event — plugin should ignore it.
    Ignored,
}

fn classify_web_event(event: &AgentEvent) -> WebEventClass<'_> {
    match event {
        AgentEvent::ToolExecutionStart { name, .. } if name.starts_with("web_") => {
            WebEventClass::Start(name.as_str())
        }
        AgentEvent::ToolExecutionEnd { name, is_error, .. }
            if *is_error && name.starts_with("web_") =>
        {
            WebEventClass::Error(name.as_str())
        }
        _ => WebEventClass::Ignored,
    }
}

#[cfg(test)]
#[path = "plugin_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "plugin_on_event_tests.rs"]
mod on_event_tests;
