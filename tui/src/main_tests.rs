//! Tests for `main`.
#![cfg(test)]

use super::*;

#[test]
fn build_stream_fn_returns_openai_for_openai_key() {
    let sfn = build_stream_fn("openai", "https://api.openai.com", "test-key");
    assert!(sfn.is_some(), "openai provider should produce a StreamFn");
    // Pins that the TUI and the preset factory agree on which wire
    // OPENAI_API names — `reasoning_effort` is only supported on Responses.
    let expected_responses = swink_agent_adapters::OpenAiWire::from_env().unwrap_or_default()
        == swink_agent_adapters::OpenAiWire::Responses;
    assert_eq!(
        sfn.unwrap().supported_serving_options().reasoning_effort,
        expected_responses
    );
}

#[test]
fn build_stream_fn_returns_anthropic_for_anthropic_key() {
    let sfn = build_stream_fn("anthropic", "https://api.anthropic.com", "test-key");
    assert!(
        sfn.is_some(),
        "anthropic provider should produce a StreamFn"
    );
}

#[test]
fn build_stream_fn_returns_none_for_unknown_provider() {
    let sfn = build_stream_fn("unknown_provider", "https://example.com", "key");
    assert!(sfn.is_none(), "unknown provider should return None");
}

#[test]
fn catalog_presets_contain_expected_providers() {
    let anthropic_presets = remote_presets(Some("anthropic"));
    assert!(
        !anthropic_presets.is_empty(),
        "catalog should have anthropic presets"
    );
    assert!(
        anthropic_presets
            .iter()
            .any(|p| p.model_id.contains("claude")),
        "anthropic presets should contain claude models"
    );

    let openai_presets = remote_presets(Some("openai"));
    assert!(
        !openai_presets.is_empty(),
        "catalog should have openai presets"
    );
    assert!(
        openai_presets.iter().any(|p| p.model_id == "gpt-5.4"),
        "openai presets should contain gpt-5.4"
    );
}

#[test]
fn catalog_presets_provide_model_specs_with_capabilities() {
    let presets = remote_presets(Some("anthropic"));
    let sonnet = presets
        .iter()
        .find(|p| p.model_id.contains("sonnet"))
        .expect("catalog should have a sonnet preset");
    let spec = sonnet.model_spec();
    assert_eq!(spec.provider, "anthropic");
    assert!(
        spec.capabilities
            .as_ref()
            .is_some_and(|c| c.supports_tool_use),
        "sonnet should support tool use"
    );
}

#[test]
fn try_proxy_returns_none_without_env_var() {
    // LLM_BASE_URL is not normally set in test environments
    if std::env::var("LLM_BASE_URL").is_err() {
        assert!(try_proxy().is_none());
    }
}

#[cfg(not(feature = "local"))]
#[test]
fn setup_wizard_runs_when_no_keys_and_no_local_provider() {
    assert!(should_run_setup_wizard_with(false));
    assert!(!should_run_setup_wizard_with(true));
}

#[cfg(feature = "local")]
#[test]
fn setup_wizard_is_skipped_when_local_feature_is_enabled() {
    assert!(!should_run_setup_wizard_with(false));
    assert!(!should_run_setup_wizard_with(true));
}

#[cfg(feature = "local")]
#[test]
fn try_local_returns_local_connection_when_feature_enabled() {
    let connections = try_local().expect("local feature should provide a default connection");
    assert_eq!(connections.primary_model().provider, "local");
    assert!(
        !connections.primary_model().model_id.is_empty(),
        "local connection should expose a catalog-backed model id"
    );
}
