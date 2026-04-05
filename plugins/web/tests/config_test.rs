use std::time::Duration;

use swink_agent_plugin_web::config::{SearchProviderKind, WebPluginConfigBuilder};

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
fn builder_overrides_rate_limit() {
    let config = WebPluginConfigBuilder::new()
        .with_rate_limit_rpm(60)
        .build();
    assert_eq!(config.rate_limit_rpm, 60);
}

#[test]
fn builder_overrides_max_content_length() {
    let config = WebPluginConfigBuilder::new()
        .with_max_content_length(100_000)
        .build();
    assert_eq!(config.max_content_length, 100_000);
}

#[test]
fn builder_overrides_block_private_ips() {
    let config = WebPluginConfigBuilder::new()
        .with_block_private_ips(false)
        .build();
    assert!(!config.block_private_ips);
}

#[test]
fn builder_overrides_user_agent() {
    let config = WebPluginConfigBuilder::new()
        .with_user_agent("TestAgent/1.0")
        .build();
    assert_eq!(config.user_agent, "TestAgent/1.0");
}

#[test]
fn builder_overrides_viewport() {
    let config = WebPluginConfigBuilder::new()
        .with_viewport(1920, 1080)
        .build();
    assert_eq!(config.viewport_width, 1920);
    assert_eq!(config.viewport_height, 1080);
}

#[test]
fn builder_overrides_search_provider() {
    let config = WebPluginConfigBuilder::new()
        .with_search_provider(SearchProviderKind::Brave)
        .with_brave_api_key("test-key")
        .build();
    assert!(matches!(
        config.search_provider_kind,
        SearchProviderKind::Brave
    ));
    assert_eq!(config.brave_api_key.as_deref(), Some("test-key"));
}

#[test]
fn builder_overrides_max_redirects() {
    let config = WebPluginConfigBuilder::new()
        .with_max_redirects(5)
        .build();
    assert_eq!(config.max_redirects, 5);
}

#[test]
fn builder_overrides_max_search_results() {
    let config = WebPluginConfigBuilder::new()
        .with_max_search_results(25)
        .build();
    assert_eq!(config.max_search_results, 25);
}

#[test]
fn builder_overrides_sanitizer_enabled() {
    let config = WebPluginConfigBuilder::new()
        .with_sanitizer_enabled(false)
        .build();
    assert!(!config.sanitizer_enabled);
}

#[test]
fn builder_overrides_timeouts() {
    let config = WebPluginConfigBuilder::new()
        .with_request_timeout(Duration::from_secs(60))
        .with_screenshot_timeout(Duration::from_secs(30))
        .build();
    assert_eq!(config.request_timeout, Duration::from_secs(60));
    assert_eq!(config.screenshot_timeout, Duration::from_secs(30));
}

#[test]
fn builder_overrides_domain_lists() {
    let config = WebPluginConfigBuilder::new()
        .with_domain_allowlist(vec!["example.com".into()])
        .with_domain_denylist(vec!["evil.com".into()])
        .build();
    assert_eq!(config.domain_allowlist, vec!["example.com"]);
    assert_eq!(config.domain_denylist, vec!["evil.com"]);
}

#[test]
fn builder_chaining_applies_all_overrides() {
    let config = WebPluginConfigBuilder::new()
        .with_rate_limit_rpm(60)
        .with_max_content_length(100_000)
        .with_block_private_ips(false)
        .with_user_agent("TestAgent/1.0")
        .with_viewport(1920, 1080)
        .with_max_redirects(3)
        .with_sanitizer_enabled(false)
        .build();

    assert_eq!(config.rate_limit_rpm, 60);
    assert_eq!(config.max_content_length, 100_000);
    assert!(!config.block_private_ips);
    assert_eq!(config.user_agent, "TestAgent/1.0");
    assert_eq!(config.viewport_width, 1920);
    assert_eq!(config.viewport_height, 1080);
    assert_eq!(config.max_redirects, 3);
    assert!(!config.sanitizer_enabled);
}
