use std::sync::OnceLock;

use serde::Deserialize;

use crate::ModelSpec;
use crate::types::{Cost, ModelCapabilities, Usage};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Remote,
    Local,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    Bearer,
    ApiKeyHeader,
    AwsSigv4,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiVersion {
    V1,
    V1beta,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetCapability {
    Text,
    Tools,
    Thinking,
    ImagesIn,
    Streaming,
    StructuredOutput,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetStatus {
    Ga,
    Preview,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PresetCatalog {
    pub id: String,
    pub display_name: String,
    pub group: Option<String>,
    pub model_id: String,
    pub api_version: Option<ApiVersion>,
    #[serde(default)]
    pub capabilities: Vec<PresetCapability>,
    pub status: Option<PresetStatus>,
    pub context_window_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
    #[serde(default)]
    pub include_by_default: bool,
    pub repo_id: Option<String>,
    pub filename: Option<String>,
    #[serde(default)]
    pub cost_per_million_input: Option<f64>,
    #[serde(default)]
    pub cost_per_million_output: Option<f64>,
    #[serde(default)]
    pub cost_per_million_cache_read: Option<f64>,
    #[serde(default)]
    pub cost_per_million_cache_write: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ProviderCatalog {
    pub key: String,
    pub display_name: String,
    pub kind: ProviderKind,
    pub auth_mode: Option<AuthMode>,
    pub credential_env_var: Option<String>,
    pub base_url_env_var: Option<String>,
    pub default_base_url: Option<String>,
    #[serde(default)]
    pub requires_base_url: bool,
    pub region_env_var: Option<String>,
    #[serde(default)]
    pub presets: Vec<PresetCatalog>,
}

impl ProviderCatalog {
    #[must_use]
    pub fn preset(&self, preset_id: &str) -> Option<&PresetCatalog> {
        self.presets.iter().find(|preset| preset.id == preset_id)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ModelCatalog {
    #[serde(default)]
    pub providers: Vec<ProviderCatalog>,
}

impl ModelCatalog {
    #[must_use]
    pub fn provider(&self, provider_key: &str) -> Option<&ProviderCatalog> {
        self.providers
            .iter()
            .find(|provider| provider.key == provider_key)
    }

    /// Search across all providers for a preset matching the given `model_id`.
    #[must_use]
    pub fn find_preset_by_model_id(&self, model_id: &str) -> Option<CatalogPreset> {
        for provider in &self.providers {
            for preset in &provider.presets {
                if preset.model_id == model_id {
                    return self.preset(&provider.key, &preset.id);
                }
            }
        }
        None
    }

    #[must_use]
    pub fn preset(&self, provider_key: &str, preset_id: &str) -> Option<CatalogPreset> {
        let provider = self.provider(provider_key)?;
        let preset = provider.preset(preset_id)?;
        Some(CatalogPreset {
            provider_key: provider.key.clone(),
            provider_display_name: provider.display_name.clone(),
            provider_kind: provider.kind.clone(),
            preset_id: preset.id.clone(),
            display_name: preset.display_name.clone(),
            group: preset.group.clone(),
            model_id: preset.model_id.clone(),
            api_version: preset.api_version.clone(),
            capabilities: preset.capabilities.clone(),
            status: preset.status.clone(),
            context_window_tokens: preset.context_window_tokens,
            max_output_tokens: preset.max_output_tokens,
            auth_mode: provider.auth_mode.clone(),
            credential_env_var: provider.credential_env_var.clone(),
            base_url_env_var: provider.base_url_env_var.clone(),
            default_base_url: provider.default_base_url.clone(),
            requires_base_url: provider.requires_base_url,
            region_env_var: provider.region_env_var.clone(),
            include_by_default: preset.include_by_default,
            repo_id: preset.repo_id.clone(),
            filename: preset.filename.clone(),
            cost_per_million_input: preset.cost_per_million_input,
            cost_per_million_output: preset.cost_per_million_output,
            cost_per_million_cache_read: preset.cost_per_million_cache_read,
            cost_per_million_cache_write: preset.cost_per_million_cache_write,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CatalogPreset {
    pub provider_key: String,
    pub provider_display_name: String,
    pub provider_kind: ProviderKind,
    pub preset_id: String,
    pub display_name: String,
    pub group: Option<String>,
    pub model_id: String,
    pub api_version: Option<ApiVersion>,
    pub capabilities: Vec<PresetCapability>,
    pub status: Option<PresetStatus>,
    pub context_window_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub auth_mode: Option<AuthMode>,
    pub credential_env_var: Option<String>,
    pub base_url_env_var: Option<String>,
    pub default_base_url: Option<String>,
    pub requires_base_url: bool,
    pub region_env_var: Option<String>,
    pub include_by_default: bool,
    pub repo_id: Option<String>,
    pub filename: Option<String>,
    pub cost_per_million_input: Option<f64>,
    pub cost_per_million_output: Option<f64>,
    pub cost_per_million_cache_read: Option<f64>,
    pub cost_per_million_cache_write: Option<f64>,
}

impl CatalogPreset {
    /// Build a [`ModelCapabilities`] from the catalog's capability list and
    /// token limits.
    #[must_use]
    pub fn model_capabilities(&self) -> ModelCapabilities {
        let has = |cap: &PresetCapability| self.capabilities.contains(cap);
        ModelCapabilities {
            supports_thinking: has(&PresetCapability::Thinking),
            supports_vision: has(&PresetCapability::ImagesIn),
            supports_tool_use: has(&PresetCapability::Tools),
            supports_streaming: has(&PresetCapability::Streaming),
            supports_structured_output: has(&PresetCapability::StructuredOutput),
            max_context_window: self.context_window_tokens,
            max_output_tokens: self.max_output_tokens,
        }
    }

    /// Create a [`ModelSpec`] pre-populated with capabilities from the catalog.
    #[must_use]
    pub fn model_spec(&self) -> ModelSpec {
        ModelSpec::new(&self.provider_key, &self.model_id)
            .with_capabilities(self.model_capabilities())
    }
}

#[must_use]
pub fn model_catalog() -> &'static ModelCatalog {
    static MODEL_CATALOG: OnceLock<ModelCatalog> = OnceLock::new();
    MODEL_CATALOG.get_or_init(|| {
        toml::from_str(include_str!("model_catalog.toml"))
            .expect("src/model_catalog.toml must be valid TOML")
    })
}

/// Compute monetary cost from token usage using catalog pricing data.
///
/// Looks up the model by `model_id` across all providers. Returns
/// `Cost::default()` if the model is not found or has no pricing data.
#[must_use]
pub fn calculate_cost(model_id: &str, usage: &Usage) -> Cost {
    let Some(preset) = model_catalog().find_preset_by_model_id(model_id) else {
        return Cost::default();
    };

    #[allow(clippy::cast_precision_loss)] // token counts fit comfortably in f64
    let per_m = |tokens: u64, rate: Option<f64>| -> f64 {
        rate.map_or(0.0, |r| tokens as f64 * r / 1_000_000.0)
    };

    let input = per_m(usage.input, preset.cost_per_million_input);
    let output = per_m(usage.output, preset.cost_per_million_output);
    let cache_read = per_m(usage.cache_read, preset.cost_per_million_cache_read);
    let cache_write = per_m(usage.cache_write, preset.cost_per_million_cache_write);

    Cost {
        input,
        output,
        cache_read,
        cache_write,
        total: input + output + cache_read + cache_write,
        ..Cost::default()
    }
}

#[cfg(test)]
mod tests {
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
        assert_eq!(google.presets.len(), 4);

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
        assert_eq!(caps.max_context_window, Some(200_000));
        assert_eq!(caps.max_output_tokens, Some(16384));
    }

    #[test]
    fn model_spec_carries_capabilities_from_preset() {
        let preset = model_catalog().preset("anthropic", "opus_46").unwrap();
        let spec = preset.model_spec();
        let caps = spec.capabilities();
        assert!(caps.supports_thinking);
        assert!(caps.supports_vision);
        assert!(caps.supports_tool_use);
        assert_eq!(caps.max_context_window, Some(200_000));
        assert_eq!(caps.max_output_tokens, Some(32768));
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
}
