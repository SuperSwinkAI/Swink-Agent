use std::sync::OnceLock;

use serde::Deserialize;

use crate::types::ModelCapabilities;
use crate::ModelSpec;

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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
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
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
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
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
        assert!(local.preset("smollm3_3b").unwrap().include_by_default);

        let google = catalog.provider("google").unwrap();
        assert_eq!(google.kind, ProviderKind::Remote);
        assert_eq!(google.presets.len(), 4);

        let bedrock = catalog.provider("bedrock").unwrap();
        assert_eq!(bedrock.auth_mode, Some(AuthMode::AwsSigv4));
        assert_eq!(bedrock.region_env_var.as_deref(), Some("AWS_REGION"));
    }

    #[test]
    fn preset_lookup_returns_provider_metadata() {
        let preset = model_catalog().preset("openai", "gpt_5_2").unwrap();
        assert_eq!(preset.display_name, "OpenAI GPT-5.2");
        assert_eq!(preset.model_id, "gpt-5.2");
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
        let azure = model_catalog().preset("azure", "gpt_5_4").unwrap();
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
        let preset = model_catalog().preset("openai", "gpt_5_2").unwrap();
        let caps = preset.model_capabilities();
        assert!(!caps.supports_thinking);
        assert!(caps.supports_tool_use);
        assert!(caps.supports_vision);
        assert!(caps.supports_streaming);
        assert!(caps.supports_structured_output);
        assert_eq!(caps.max_context_window, Some(128_000));
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
}
