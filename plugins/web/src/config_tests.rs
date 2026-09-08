//! Tests for `config`.
#![cfg(test)]

use super::*;

#[test]
fn default_config_has_expected_values() {
    let config = WebPluginConfigBuilder::new().build();
    assert!(matches!(
        config.search_provider_kind,
        SearchProviderKind::DuckDuckGo
    ));
    assert!(config.block_private_ips);
    assert_eq!(config.rate_limit_rpm, 30);
    assert_eq!(config.max_content_length, 50_000);
    assert_eq!(config.max_redirects, 10);
    assert_eq!(config.max_search_results, 10);
    assert!(config.sanitizer_enabled);
    assert_eq!(config.user_agent, "SwinkAgent/0.5");
    assert_eq!(config.viewport_width, 1280);
    assert_eq!(config.viewport_height, 720);
    assert!(config.domain_allowlist.is_empty());
    assert!(config.domain_denylist.is_empty());
    assert!(config.brave_api_key.is_none());
    assert!(config.tavily_api_key.is_none());
    assert!(config.playwright_path.is_none());
    assert_eq!(config.screenshot_timeout, Duration::from_secs(15));
    assert_eq!(config.request_timeout, Duration::from_secs(30));
}

#[test]
fn builder_chaining_applies_overrides() {
    let config = WebPluginConfigBuilder::new()
        .with_search_provider(SearchProviderKind::Brave)
        .with_brave_api_key("test-key")
        .with_rate_limit_rpm(60)
        .with_max_content_length(100_000)
        .with_block_private_ips(false)
        .with_user_agent("TestAgent/1.0")
        .with_viewport(1920, 1080)
        .with_max_redirects(3)
        .with_max_search_results(25)
        .with_sanitizer_enabled(false)
        .with_request_timeout(Duration::from_mins(1))
        .with_screenshot_timeout(Duration::from_secs(30))
        .with_domain_allowlist(vec!["example.com".into()])
        .with_domain_denylist(vec!["evil.com".into()])
        .build();

    assert!(matches!(
        config.search_provider_kind,
        SearchProviderKind::Brave
    ));
    assert_eq!(config.brave_api_key.as_deref(), Some("test-key"));
    assert_eq!(config.rate_limit_rpm, 60);
    assert_eq!(config.max_content_length, 100_000);
    assert!(!config.block_private_ips);
    assert_eq!(config.user_agent, "TestAgent/1.0");
    assert_eq!(config.viewport_width, 1920);
    assert_eq!(config.viewport_height, 1080);
    assert_eq!(config.max_redirects, 3);
    assert_eq!(config.max_search_results, 25);
    assert!(!config.sanitizer_enabled);
    assert_eq!(config.request_timeout, Duration::from_mins(1));
    assert_eq!(config.screenshot_timeout, Duration::from_secs(30));
    assert_eq!(config.domain_allowlist, vec!["example.com"]);
    assert_eq!(config.domain_denylist, vec!["evil.com"]);
}

#[test]
fn debug_output_redacts_search_provider_api_keys() {
    const BRAVE_SECRET: &str = "brave-super-secret-token";
    const TAVILY_SECRET: &str = "tavily-super-secret-token";
    let config = WebPluginConfigBuilder::new()
        .with_brave_api_key(BRAVE_SECRET)
        .with_tavily_api_key(TAVILY_SECRET)
        .build();

    let debug = format!("{config:?}");

    assert!(!debug.contains(BRAVE_SECRET), "brave key leaked: {debug}");
    assert!(!debug.contains(TAVILY_SECRET), "tavily key leaked: {debug}");
    assert_eq!(debug.matches("[REDACTED]").count(), 2, "{debug}");
}
