//! Tests for `model_catalog`.
#![cfg(test)]

use super::*;

#[test]
fn catalog_loads_grouped_presets() {
    let catalog = model_catalog();
    let anthropic = catalog.provider("anthropic").unwrap();
    assert_eq!(anthropic.kind, ProviderKind::Remote);
    assert!(anthropic.preset("sonnet_46").is_some());

    let local = catalog.provider("local").unwrap();
    assert_eq!(local.kind, ProviderKind::Local);
    assert!(!local.preset("smollm3_3b").unwrap().include_by_default);
    assert!(local.preset("gemma4_e2b").unwrap().include_by_default);
    assert_eq!(
        local.preset("gemma4_e2b").unwrap().context_window_tokens,
        Some(128_000)
    );

    let google = catalog.provider("google").unwrap();
    assert_eq!(google.kind, ProviderKind::Remote);
    assert_eq!(google.presets.len(), 5);

    let bedrock = catalog.provider("bedrock").unwrap();
    assert_eq!(bedrock.auth_mode, Some(AuthMode::AwsSigv4));
    assert_eq!(bedrock.region_env_var.as_deref(), Some("AWS_REGION"));
}

#[test]
fn preset_lookup_returns_provider_metadata() {
    let preset = model_catalog().preset("openai", "gpt_5_4").unwrap();
    assert_eq!(preset.display_name, "OpenAI GPT-5.4");
    assert_eq!(preset.model_id, "gpt-5.4");
    assert_eq!(preset.credential_env_var.as_deref(), Some("OPENAI_API_KEY"));
    assert_eq!(preset.base_url_env_var.as_deref(), Some("OPENAI_BASE_URL"));
    assert_eq!(preset.auth_mode, Some(AuthMode::Bearer));
}

#[test]
fn google_preset_lookup_returns_extended_metadata() {
    let preset = model_catalog().preset("google", "gemini_3_flash").unwrap();
    assert_eq!(preset.display_name, "Google Gemini 3 Flash");
    assert_eq!(preset.model_id, "gemini-3-flash-preview");
    assert_eq!(preset.api_version, Some(ApiVersion::V1beta));
    assert_eq!(preset.status, Some(PresetStatus::Preview));
    assert_eq!(
        preset.capabilities,
        vec![
            PresetCapability::Text,
            PresetCapability::Tools,
            PresetCapability::Thinking,
            PresetCapability::ImagesIn,
            PresetCapability::Streaming,
            PresetCapability::StructuredOutput,
        ]
    );
    assert_eq!(preset.context_window_tokens, Some(1_000_000));
    assert_eq!(preset.max_output_tokens, Some(65536));
    assert_eq!(preset.credential_env_var.as_deref(), Some("GEMINI_API_KEY"));
    assert_eq!(preset.base_url_env_var.as_deref(), Some("GEMINI_BASE_URL"));
}

#[test]
fn azure_and_bedrock_presets_expose_provider_specific_metadata() {
    let azure = model_catalog().preset("azure", "gpt_4o").unwrap();
    assert_eq!(azure.auth_mode, Some(AuthMode::ApiKeyHeader));
    assert!(azure.requires_base_url);
    assert_eq!(azure.base_url_env_var.as_deref(), Some("AZURE_BASE_URL"));

    let bedrock = model_catalog()
        .preset("bedrock", "anthropic_claude_sonnet_45")
        .unwrap();
    assert_eq!(bedrock.auth_mode, Some(AuthMode::AwsSigv4));
    assert_eq!(bedrock.region_env_var.as_deref(), Some("AWS_REGION"));
    assert_eq!(bedrock.group.as_deref(), Some("anthropic"));
}

#[test]
fn anthropic_preset_model_capabilities() {
    let preset = model_catalog().preset("anthropic", "sonnet_46").unwrap();
    let caps = preset.model_capabilities();
    assert!(caps.supports_thinking);
    assert!(caps.supports_vision);
    assert!(caps.supports_tool_use);
    assert!(caps.supports_streaming);
    assert!(caps.supports_structured_output);
    assert_eq!(caps.max_context_window, Some(1_000_000));
    assert_eq!(caps.max_output_tokens, Some(65536));
}

#[test]
fn model_spec_carries_capabilities_from_preset() {
    let preset = model_catalog().preset("anthropic", "opus_46").unwrap();
    let spec = preset.model_spec();
    let caps = spec.capabilities();
    assert!(caps.supports_thinking);
    assert!(caps.supports_vision);
    assert!(caps.supports_tool_use);
    assert_eq!(caps.max_context_window, Some(1_000_000));
    assert_eq!(caps.max_output_tokens, Some(131_072));
}

#[test]
fn openai_preset_no_thinking() {
    let preset = model_catalog().preset("openai", "gpt_5_4_mini").unwrap();
    let caps = preset.model_capabilities();
    assert!(!caps.supports_thinking);
    assert!(caps.supports_tool_use);
    assert!(caps.supports_vision);
    assert!(caps.supports_streaming);
    assert!(caps.supports_structured_output);
    assert_eq!(caps.max_context_window, Some(400_000));
}

#[test]
fn local_preset_minimal_capabilities() {
    let preset = model_catalog().preset("local", "smollm3_3b").unwrap();
    let caps = preset.model_capabilities();
    assert!(!caps.supports_thinking);
    assert!(!caps.supports_vision);
    assert!(!caps.supports_tool_use);
    assert!(caps.supports_streaming);
    assert!(!caps.supports_structured_output);
    assert_eq!(caps.max_context_window, Some(8192));
    assert_eq!(caps.max_output_tokens, Some(2048));
}

#[test]
fn bedrock_preset_capabilities() {
    let preset = model_catalog()
        .preset("bedrock", "anthropic_claude_sonnet_45")
        .unwrap();
    let caps = preset.model_capabilities();
    assert!(caps.supports_thinking);
    assert!(caps.supports_vision);
    assert!(caps.supports_tool_use);
    assert!(caps.supports_streaming);
    assert!(!caps.supports_structured_output);
}

#[test]
fn local_thinking_preset_model_spec_defaults_to_thinking_on() {
    let preset = model_catalog().preset("local", "gemma4_e2b").unwrap();
    let spec = preset.model_spec();
    assert!(spec.capabilities().supports_thinking);
    assert_ne!(spec.thinking_level, ThinkingLevel::Off);
}

#[test]
fn local_non_thinking_preset_model_spec_stays_off() {
    let preset = model_catalog().preset("local", "smollm3_3b").unwrap();
    let spec = preset.model_spec();
    assert!(!spec.capabilities().supports_thinking);
    assert_eq!(spec.thinking_level, ThinkingLevel::Off);
}

#[test]
fn remote_thinking_preset_model_spec_stays_opt_in() {
    let preset = model_catalog().preset("anthropic", "sonnet_46").unwrap();
    let spec = preset.model_spec();
    assert!(spec.capabilities().supports_thinking);
    assert_eq!(spec.thinking_level, ThinkingLevel::Off);
}

#[test]
fn local_thinking_default_can_be_explicitly_disabled() {
    let preset = model_catalog().preset("local", "gemma4_e2b").unwrap();
    let spec = preset.model_spec().with_thinking_level(ThinkingLevel::Off);
    assert_eq!(spec.thinking_level, ThinkingLevel::Off);
}

#[test]
fn manual_model_spec_defaults_to_no_capabilities() {
    let spec = crate::ModelSpec::new("custom", "my-model");
    let caps = spec.capabilities();
    assert!(!caps.supports_thinking);
    assert!(!caps.supports_vision);
    assert!(!caps.supports_tool_use);
    assert!(!caps.supports_streaming);
    assert!(!caps.supports_structured_output);
    assert_eq!(caps.max_context_window, None);
    assert_eq!(caps.max_output_tokens, None);
}

// --- US4: Cost calculation tests ---

fn usage(input: u64, output: u64, cache_read: u64, cache_write: u64) -> crate::types::Usage {
    crate::types::Usage {
        input,
        output,
        cache_read,
        cache_write,
        total: input + output + cache_read + cache_write,
        ..Default::default()
    }
}

#[test]
fn calculate_cost_known_model() {
    // Sonnet 4.6: input=$3/M, output=$15/M
    let cost = calculate_cost("claude-sonnet-4-6", &usage(1_000_000, 500_000, 0, 0));
    assert!((cost.input - 3.0).abs() < 0.001);
    assert!((cost.output - 7.5).abs() < 0.001);
    assert!((cost.total - 10.5).abs() < 0.001);
}

#[test]
fn calculate_cost_unknown_model() {
    let cost = calculate_cost("nonexistent-model-xyz", &usage(1_000_000, 1_000_000, 0, 0));
    assert!((cost.input).abs() < 0.001);
    assert!((cost.output).abs() < 0.001);
    assert!((cost.total).abs() < 0.001);
}

#[test]
fn calculate_cost_zero_usage() {
    let cost = calculate_cost("claude-sonnet-4-6", &usage(0, 0, 0, 0));
    assert!((cost.total).abs() < 0.001);
}

fn message(model_id: &str, usage: Usage, cost: Cost) -> AssistantMessage {
    AssistantMessage {
        content: vec![],
        provider: "anthropic".to_string(),
        model_id: model_id.to_string(),
        usage,
        cost,
        stop_reason: crate::types::StopReason::Stop,
        error_message: None,
        error_kind: None,
        timestamp: 0,
        cache_hint: None,
    }
}

#[test]
fn price_assistant_message_fills_in_unpriced_message() {
    let mut msg = message(
        "claude-sonnet-4-6",
        usage(1_000_000, 1_000_000, 0, 0),
        Cost::default(),
    );
    assert!(price_assistant_message(&mut msg));
    assert!((msg.cost.input - 3.0).abs() < 0.001);
    assert!((msg.cost.total - msg.cost.input - msg.cost.output).abs() < 0.001);
    assert!(msg.cost.total > 0.0);
}

#[test]
fn price_assistant_message_preserves_adapter_supplied_cost() {
    let adapter_cost = Cost {
        input: 0.5,
        total: 0.5,
        ..Cost::default()
    };
    let mut msg = message(
        "claude-sonnet-4-6",
        usage(1_000_000, 1_000_000, 0, 0),
        adapter_cost,
    );
    assert!(!price_assistant_message(&mut msg));
    assert!((msg.cost.total - 0.5).abs() < 0.001);
}

#[test]
fn price_assistant_message_leaves_unknown_model_at_zero() {
    let mut msg = message(
        "nonexistent-model-xyz",
        usage(1_000_000, 1_000_000, 0, 0),
        Cost::default(),
    );
    assert!(!price_assistant_message(&mut msg));
    assert!(msg.cost.is_zero());
}

#[test]
fn price_assistant_message_leaves_zero_usage_at_zero() {
    let mut msg = message("claude-sonnet-4-6", usage(0, 0, 0, 0), Cost::default());
    assert!(!price_assistant_message(&mut msg));
    assert!(msg.cost.is_zero());
}

/// Issue #1084: operator-declared rates must beat the compiled catalog.
///
/// `claude-sonnet-4-6` is in the catalog at $3.00/M input. An operator who
/// declares $1.00/M must see $1.00 — otherwise a `[pricing]` config section
/// silently does nothing for any model the catalog happens to know.
#[test]
fn operator_declared_rates_take_precedence_over_catalog() {
    let table = crate::pricing::PricingTable::new().with_model(
        "claude-sonnet-4-6",
        crate::pricing::ModelRates {
            input_per_million: 1.0,
            ..crate::pricing::ModelRates::default()
        },
    );
    let mut msg = message(
        "claude-sonnet-4-6",
        usage(1_000_000, 0, 0, 0),
        Cost::default(),
    );

    assert!(price_assistant_message_with(&mut msg, Some(&table)));
    assert!(
        (msg.cost.total - 1.0).abs() < 0.001,
        "expected the operator's $1.00/M rate, got ${:.4} (catalog rate is $3.00/M)",
        msg.cost.total
    );
}

/// A calculator that declines a model must not suppress catalog pricing.
#[test]
fn calculator_declining_a_model_falls_back_to_catalog() {
    let table = crate::pricing::PricingTable::new().with_model(
        "some-other-model",
        crate::pricing::ModelRates {
            input_per_million: 1.0,
            ..crate::pricing::ModelRates::default()
        },
    );
    let mut msg = message(
        "claude-sonnet-4-6",
        usage(1_000_000, 0, 0, 0),
        Cost::default(),
    );

    assert!(price_assistant_message_with(&mut msg, Some(&table)));
    assert!(
        (msg.cost.total - 3.0).abs() < 0.001,
        "expected catalog pricing"
    );
}

/// Operator rates are the only way to price a model the catalog has never
/// heard of — local endpoints and private deployments.
#[test]
fn operator_declared_rates_price_a_model_absent_from_the_catalog() {
    let table = crate::pricing::PricingTable::new().with_model(
        "my-local-llama",
        crate::pricing::ModelRates {
            input_per_million: 0.10,
            output_per_million: 0.40,
            ..crate::pricing::ModelRates::default()
        },
    );
    let mut msg = message(
        "my-local-llama",
        usage(1_000_000, 1_000_000, 0, 0),
        Cost::default(),
    );

    assert!(price_assistant_message_with(&mut msg, Some(&table)));
    assert!((msg.cost.total - 0.50).abs() < 0.001);
}

/// The adapter's own billed cost outranks even an operator override.
#[test]
fn adapter_supplied_cost_outranks_operator_declared_rates() {
    let table = crate::pricing::PricingTable::new().with_model(
        "claude-sonnet-4-6",
        crate::pricing::ModelRates {
            input_per_million: 1.0,
            ..crate::pricing::ModelRates::default()
        },
    );
    let adapter_cost = Cost {
        input: 0.25,
        total: 0.25,
        ..Cost::default()
    };
    let mut msg = message("claude-sonnet-4-6", usage(1_000_000, 0, 0, 0), adapter_cost);

    assert!(!price_assistant_message_with(&mut msg, Some(&table)));
    assert!((msg.cost.total - 0.25).abs() < 0.001);
}

/// A calculator returning an explicit zero declines rather than pinning the
/// message to zero, so the catalog still gets a turn.
#[test]
fn calculator_returning_zero_cost_falls_back_to_catalog() {
    let zeroing = |_model_id: &str, _usage: &Usage| Some(Cost::default());
    let mut msg = message(
        "claude-sonnet-4-6",
        usage(1_000_000, 0, 0, 0),
        Cost::default(),
    );

    assert!(price_assistant_message_with(&mut msg, Some(&zeroing)));
    assert!((msg.cost.total - 3.0).abs() < 0.001);
}

#[test]
fn calculate_cost_cache_tokens() {
    // Sonnet 4.6: cache_read=$0.30/M, cache_write=$3.75/M
    let cost = calculate_cost("claude-sonnet-4-6", &usage(0, 0, 2_000_000, 1_000_000));
    assert!((cost.cache_read - 0.60).abs() < 0.001);
    assert!((cost.cache_write - 3.75).abs() < 0.001);
    assert!((cost.total - 4.35).abs() < 0.001);
}

#[test]
fn calculate_cost_no_pricing_data() {
    // Local model has no pricing fields
    let cost = calculate_cost("SmolLM3-3B-Q4_K_M", &usage(1_000_000, 500_000, 0, 0));
    assert!((cost.total).abs() < 0.001);
}

// --- US5: Capability introspection tests ---

#[test]
fn capabilities_from_catalog_preset() {
    let preset = model_catalog().preset("anthropic", "sonnet_46").unwrap();
    let caps = preset.model_capabilities();
    assert!(caps.supports_thinking);
    assert!(caps.supports_vision);
    assert!(caps.supports_tool_use);
    assert!(caps.supports_streaming);
    assert!(caps.supports_structured_output);
}

#[test]
fn capabilities_context_window_and_output() {
    let preset = model_catalog().preset("openai", "gpt_5_4").unwrap();
    let caps = preset.model_capabilities();
    assert_eq!(caps.max_context_window, Some(1_050_000));
    assert_eq!(caps.max_output_tokens, Some(128_000));
}

#[test]
fn model_spec_carries_capabilities() {
    let preset = model_catalog().preset("google", "gemini_3_flash").unwrap();
    let spec = preset.model_spec();
    let caps = spec.capabilities();
    assert!(caps.supports_thinking);
    assert!(caps.supports_vision);
    assert!(caps.supports_tool_use);
    assert_eq!(caps.max_context_window, Some(1_000_000));
}

#[test]
fn find_preset_by_model_id_works() {
    let preset = model_catalog()
        .find_preset_by_model_id("claude-sonnet-4-6")
        .unwrap();
    assert_eq!(preset.preset_id, "sonnet_46");
    assert_eq!(preset.provider_key, "anthropic");
}

#[test]
fn find_preset_by_model_id_unknown_returns_none() {
    assert!(
        model_catalog()
            .find_preset_by_model_id("nonexistent")
            .is_none()
    );
}

// --- Deprecation status ---

const DEPRECATED_CATALOG: &str = r#"
        pricing_as_of = "2026-01-01"

        [[providers]]
        key = "test"
        display_name = "Test Provider"
        kind = "remote"

        [[providers.presets]]
        id = "old_model"
        display_name = "Old Model"
        model_id = "old-model-1"
        status = { deprecated = { replacement_model_id = "new-model-2" } }

        [[providers.presets]]
        id = "sunset_model"
        display_name = "Sunset Model"
        model_id = "sunset-model-1"
        status = { deprecated = {} }

        [[providers.presets]]
        id = "current_model"
        display_name = "Current Model"
        model_id = "new-model-2"
        status = "ga"
    "#;

#[test]
fn deprecated_catalog_entry_parses_with_replacement_id() {
    let catalog: ModelCatalog = toml::from_str(DEPRECATED_CATALOG).unwrap();
    let preset = catalog.preset("test", "old_model").unwrap();
    assert_eq!(
        preset.status,
        Some(PresetStatus::Deprecated {
            replacement_model_id: Some("new-model-2".to_string()),
        })
    );
    assert!(preset.is_deprecated());
    assert_eq!(preset.replacement_model_id(), Some("new-model-2"));
}

#[test]
fn deprecated_catalog_entry_parses_without_replacement_id() {
    let catalog: ModelCatalog = toml::from_str(DEPRECATED_CATALOG).unwrap();
    let preset = catalog.preset("test", "sunset_model").unwrap();
    assert_eq!(
        preset.status,
        Some(PresetStatus::Deprecated {
            replacement_model_id: None,
        })
    );
    assert!(preset.is_deprecated());
    assert_eq!(preset.replacement_model_id(), None);
}

#[test]
fn string_statuses_remain_backward_compatible() {
    let catalog: ModelCatalog = toml::from_str(DEPRECATED_CATALOG).unwrap();
    let preset = catalog.preset("test", "current_model").unwrap();
    assert_eq!(preset.status, Some(PresetStatus::Ga));
    assert!(!preset.is_deprecated());
    assert_eq!(preset.replacement_model_id(), None);
}

#[test]
fn compiled_catalog_replacements_name_a_live_model() {
    // Every deprecated preset that records a successor must point at a
    // model_id the same provider still lists as non-deprecated, so a
    // stale replacement can't silently send callers to another dead id.
    let compiled = model_catalog();
    for provider in &compiled.providers {
        for preset in &provider.presets {
            let Some(replacement) = preset.status.as_ref().and_then(|status| match status {
                PresetStatus::Deprecated {
                    replacement_model_id,
                } => replacement_model_id.as_deref(),
                _ => None,
            }) else {
                continue;
            };
            assert!(
                provider.presets.iter().any(|candidate| {
                    candidate.model_id == replacement
                        && !candidate
                            .status
                            .as_ref()
                            .is_some_and(PresetStatus::is_deprecated)
                }),
                "{}.{} points at unknown or deprecated replacement {replacement}",
                provider.key,
                preset.id
            );
        }
    }
}

// --- Codex provider block (#1267) ---

#[test]
fn codex_provider_has_no_credential_env_var_and_zero_pricing() {
    let catalog = model_catalog();
    let codex = catalog.provider("codex").expect("codex provider present");
    assert_eq!(codex.kind, ProviderKind::Remote);
    assert_eq!(
        codex.credential_env_var, None,
        "auth is OAuth, not an env var"
    );
    assert_eq!(
        codex.default_base_url.as_deref(),
        Some("https://chatgpt.com/backend-api/codex")
    );
    assert!(!codex.presets.is_empty());
    for preset in &codex.presets {
        assert_eq!(preset.cost_per_million_input, Some(0.0), "{}", preset.id);
        assert_eq!(preset.cost_per_million_output, Some(0.0), "{}", preset.id);
    }
}

#[test]
fn same_slug_resolves_to_codex_or_openai_by_provider() {
    let catalog = model_catalog();
    let codex = catalog.find_preset("codex", "gpt-5.6-luna").unwrap();
    assert_eq!(codex.provider_key, "codex");
    let openai = catalog.find_preset("openai", "gpt-5.6-luna").unwrap();
    assert_eq!(openai.provider_key, "openai");
    assert!(openai.cost_per_million_input.unwrap() > 0.0);
    // The provider-blind lookup keeps its historical answer: the metered row.
    assert_eq!(
        catalog
            .find_preset_by_model_id("gpt-5.6-luna")
            .unwrap()
            .provider_key,
        "openai"
    );
    assert!(catalog.find_preset("codex", "claude-opus-5").is_none());
}

#[test]
fn calculate_cost_for_provider_prices_codex_at_zero_and_openai_at_list() {
    let usage = Usage::default()
        .with_input(1_000_000)
        .with_output(1_000_000);
    assert!(calculate_cost_for_provider("codex", "gpt-5.6-luna", &usage).is_zero());
    let openai = calculate_cost_for_provider("openai", "gpt-5.6-luna", &usage);
    assert!((openai.total - 1.40).abs() < 1e-9, "{openai:?}");
    // Provider-blind and unknown-provider both fall back to the first row.
    assert!((calculate_cost("gpt-5.6-luna", &usage).total - 1.40).abs() < 1e-9);
    assert!(
        (calculate_cost_for_provider("nope", "gpt-5.6-luna", &usage).total - 1.40).abs() < 1e-9
    );
}

#[test]
fn price_assistant_message_respects_the_message_provider() {
    let usage = Usage::default().with_input(1_000_000);
    let mut codex = AssistantMessage::new(vec![], "codex", "gpt-5.6-luna")
        .with_usage(usage.clone())
        .with_stop_reason(crate::types::StopReason::Stop)
        .with_timestamp(0);
    assert!(
        !price_assistant_message(&mut codex),
        "a $0 turn is not repriced"
    );
    assert!(codex.cost.is_zero());

    let mut openai = AssistantMessage::new(vec![], "openai", "gpt-5.6-luna")
        .with_usage(usage)
        .with_stop_reason(crate::types::StopReason::Stop)
        .with_timestamp(0);
    assert!(price_assistant_message(&mut openai));
    assert!((openai.cost.total - 0.20).abs() < 1e-9, "{:?}", openai.cost);
}

// --- Pricing staleness ---

#[test]
fn pricing_staleness_triggers_past_threshold() {
    let catalog: ModelCatalog = toml::from_str(DEPRECATED_CATALOG).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 8, 1).unwrap();
    let staleness = catalog.pricing_staleness_at(today, 180).unwrap();
    assert_eq!(
        staleness.as_of,
        NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()
    );
    assert_eq!(staleness.age_days, 212);
    assert_eq!(staleness.threshold_days, 180);
}

#[test]
fn pricing_staleness_not_triggered_before_threshold() {
    let catalog: ModelCatalog = toml::from_str(DEPRECATED_CATALOG).unwrap();
    // 31 days old — under a 180-day threshold.
    let today = NaiveDate::from_ymd_opt(2026, 2, 1).unwrap();
    assert!(catalog.pricing_staleness_at(today, 180).is_none());
    // Exactly at the threshold is still fresh (strictly greater triggers).
    let today = NaiveDate::from_ymd_opt(2026, 6, 30).unwrap();
    assert!(catalog.pricing_staleness_at(today, 180).is_none());
    // One day past the threshold triggers.
    let today = NaiveDate::from_ymd_opt(2026, 7, 1).unwrap();
    assert!(catalog.pricing_staleness_at(today, 181).is_none());
    assert!(catalog.pricing_staleness_at(today, 180).is_some());
}

#[test]
fn pricing_staleness_none_when_date_absent_or_malformed() {
    let today = NaiveDate::from_ymd_opt(2030, 1, 1).unwrap();
    let absent: ModelCatalog = toml::from_str("").unwrap();
    assert!(absent.pricing_as_of_date().is_none());
    assert!(absent.pricing_staleness_at(today, 0).is_none());

    let malformed: ModelCatalog = toml::from_str("pricing_as_of = \"soonish\"").unwrap();
    assert!(malformed.pricing_as_of_date().is_none());
    assert!(malformed.pricing_staleness_at(today, 0).is_none());
}

#[test]
fn compiled_catalog_carries_parseable_pricing_as_of() {
    assert!(
        model_catalog().pricing_as_of_date().is_some(),
        "src/model_catalog.toml must set a valid pricing_as_of date"
    );
}
