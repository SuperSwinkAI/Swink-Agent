//! Tests for `plugin`.
#![cfg(test)]

use super::*;

#[test]
fn new_with_default_config_succeeds_when_default_provider_feature_enabled() {
    // The default config uses DuckDuckGo which is enabled by default.
    assert!(WebPlugin::new().is_ok(), "default construction failed");
}

#[cfg(feature = "brave")]
#[test]
fn brave_without_api_key_returns_missing_api_key_error() {
    let config = WebPluginConfig {
        search_provider_kind: SearchProviderKind::Brave,
        brave_api_key: None,
        ..WebPluginConfig::default()
    };
    match WebPlugin::from_config(config) {
        Err(WebPluginError::MissingApiKey { provider: "brave" }) => {}
        Err(other) => panic!("unexpected error: {other:?}"),
        Ok(_) => panic!("expected missing API key error, got Ok"),
    }
}

#[cfg(feature = "tavily")]
#[test]
fn tavily_without_api_key_returns_missing_api_key_error() {
    let config = WebPluginConfig {
        search_provider_kind: SearchProviderKind::Tavily,
        tavily_api_key: None,
        ..WebPluginConfig::default()
    };
    match WebPlugin::from_config(config) {
        Err(WebPluginError::MissingApiKey { provider: "tavily" }) => {}
        Err(other) => panic!("unexpected error: {other:?}"),
        Ok(_) => panic!("expected missing API key error, got Ok"),
    }
}

#[cfg(feature = "brave")]
#[test]
fn brave_with_api_key_constructs_successfully() {
    let config = WebPluginConfig {
        search_provider_kind: SearchProviderKind::Brave,
        brave_api_key: Some("test-key".to_string()),
        ..WebPluginConfig::default()
    };
    assert!(WebPlugin::from_config(config).is_ok());
}
