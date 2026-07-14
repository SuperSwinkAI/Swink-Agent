use std::sync::OnceLock;

use chrono::NaiveDate;
use serde::Deserialize;

use crate::ModelSpec;
use crate::types::{AssistantMessage, Cost, ModelCapabilities, ThinkingLevel, Usage};

/// Whether a provider's models run on a remote API or on local hardware.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Remote,
    Local,
}

/// How requests to a provider are authenticated.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    Bearer,
    ApiKeyHeader,
    AwsSigv4,
}

/// Provider API version selector used when building request URLs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiVersion {
    V1,
    V1beta,
}

/// A capability a preset's model supports, as declared in the catalog TOML.
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

/// Release maturity of a preset's model at the provider.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetStatus {
    Ga,
    Preview,
    /// The provider has retired (or announced retirement of) this model.
    ///
    /// The preset stays listed so catalog lookups and cost calculation keep
    /// working for historical data, and `replacement_model_id` points
    /// consumers at the successor model when one is known.
    ///
    /// TOML representation (existing string statuses are unaffected):
    ///
    /// ```toml
    /// [providers.presets.status.deprecated]
    /// replacement_model_id = "gpt-5.4"
    /// ```
    Deprecated {
        #[serde(default)]
        replacement_model_id: Option<String>,
    },
}

impl PresetStatus {
    /// Returns `true` for [`PresetStatus::Deprecated`], regardless of whether
    /// a replacement model is recorded.
    #[must_use]
    pub const fn is_deprecated(&self) -> bool {
        matches!(self, Self::Deprecated { .. })
    }
}

/// A single named model preset within a [`ProviderCatalog`], as loaded from the catalog TOML.
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

/// A provider entry in the model catalog, holding its auth/connection settings and presets.
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

/// The full model catalog: a list of providers, each with its own presets.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ModelCatalog {
    /// Date (`YYYY-MM-DD`) the compiled-in pricing table was last verified
    /// against provider published prices. Used by the pricing-staleness
    /// warning at agent construction; `None` disables the check.
    #[serde(default)]
    pub pricing_as_of: Option<String>,
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

/// A preset flattened together with its parent provider's fields, for standalone use
/// once resolved via [`ModelCatalog::preset`].
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
    ///
    /// Local thinking-capable models default to [`ThinkingLevel::Medium`] so
    /// thinking is active out of the box (local inference treats any non-`Off`
    /// level as a binary "on" toggle). Remote presets keep the opt-in
    /// [`ThinkingLevel::Off`] default because remote thinking consumes billable
    /// token budget. Callers can still disable thinking explicitly via
    /// [`ModelSpec::with_thinking_level`] with [`ThinkingLevel::Off`].
    #[must_use]
    pub fn model_spec(&self) -> ModelSpec {
        let capabilities = self.model_capabilities();
        let mut spec = ModelSpec::new(&self.provider_key, &self.model_id);
        if self.provider_kind == ProviderKind::Local && capabilities.supports_thinking {
            spec = spec.with_thinking_level(ThinkingLevel::Medium);
        }
        spec.with_capabilities(capabilities)
    }

    /// Returns `true` when the preset's status is [`PresetStatus::Deprecated`].
    #[must_use]
    pub fn is_deprecated(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(PresetStatus::is_deprecated)
    }

    /// The catalog-recorded replacement for a deprecated preset, if any.
    ///
    /// Returns `None` for non-deprecated presets and for deprecated presets
    /// without a known successor.
    #[must_use]
    pub fn replacement_model_id(&self) -> Option<&str> {
        match self.status.as_ref()? {
            PresetStatus::Deprecated {
                replacement_model_id,
            } => replacement_model_id.as_deref(),
            _ => None,
        }
    }
}

impl ModelCatalog {
    /// The parsed `pricing_as_of` date, or `None` if absent or malformed.
    #[must_use]
    pub fn pricing_as_of_date(&self) -> Option<NaiveDate> {
        NaiveDate::parse_from_str(self.pricing_as_of.as_deref()?, "%Y-%m-%d").ok()
    }

    /// Check whether the catalog's pricing data is stale as of `today`.
    ///
    /// Returns `Some(PricingStaleness)` when the pricing table is older than
    /// `threshold_days`, and `None` when it is fresh or when the catalog
    /// carries no (parseable) `pricing_as_of` date.
    #[must_use]
    pub fn pricing_staleness_at(
        &self,
        today: NaiveDate,
        threshold_days: u32,
    ) -> Option<PricingStaleness> {
        let as_of = self.pricing_as_of_date()?;
        let age_days = (today - as_of).num_days();
        (age_days > i64::from(threshold_days)).then_some(PricingStaleness {
            as_of,
            age_days,
            threshold_days,
        })
    }
}

/// Default staleness threshold (in days) for the compiled-in pricing table.
pub const DEFAULT_PRICING_STALENESS_DAYS: u32 = 180;

/// Environment variable that overrides [`DEFAULT_PRICING_STALENESS_DAYS`]
/// for the warning logged at agent construction. Value is a day count.
pub const PRICING_STALENESS_ENV_VAR: &str = "SWINK_PRICING_STALENESS_DAYS";

/// Details of a stale compiled-in pricing table.
///
/// Produced by [`pricing_staleness`] / [`ModelCatalog::pricing_staleness_at`]
/// when the catalog's `pricing_as_of` date is older than the threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PricingStaleness {
    /// Date the pricing table was last verified.
    pub as_of: NaiveDate,
    /// Age of the pricing table in days, relative to the evaluation date.
    pub age_days: i64,
    /// The threshold that was exceeded.
    pub threshold_days: u32,
}

/// Check the compiled-in catalog's pricing staleness against today's date.
///
/// Returns `Some` when the pricing table is older than `threshold_days`.
/// See [`DEFAULT_PRICING_STALENESS_DAYS`] for the default threshold used at
/// agent construction.
#[must_use]
pub fn pricing_staleness(threshold_days: u32) -> Option<PricingStaleness> {
    model_catalog().pricing_staleness_at(chrono::Utc::now().date_naive(), threshold_days)
}

/// Log a once-per-process warning when the compiled-in pricing table is
/// older than the configured threshold.
///
/// The threshold defaults to [`DEFAULT_PRICING_STALENESS_DAYS`] and can be
/// overridden via the [`PRICING_STALENESS_ENV_VAR`] environment variable.
/// Called at agent construction.
pub(crate) fn warn_if_pricing_stale() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let threshold_days = std::env::var(PRICING_STALENESS_ENV_VAR)
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            .unwrap_or(DEFAULT_PRICING_STALENESS_DAYS);
        if let Some(staleness) = pricing_staleness(threshold_days) {
            tracing::warn!(
                pricing_as_of = %staleness.as_of,
                age_days = staleness.age_days,
                threshold_days = staleness.threshold_days,
                "compiled-in model pricing table may be stale; costs from \
                 calculate_cost() may not match current provider prices"
            );
        }
    });
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
        tracing::debug!(
            model_id,
            "model not found in catalog; cost reported as zero"
        );
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

/// Fill in an assistant message's [`Cost`] from catalog pricing when the
/// adapter did not price the response itself.
///
/// Most built-in remote adapters emit `Cost::default()` on every assistant
/// message — they report token [`Usage`] but leave pricing to the caller. The
/// agent loop calls this helper on each assistant message before accumulating
/// cost, so that [`PolicyContext::accumulated_cost`](crate::PolicyContext) —
/// and therefore any cost ceiling built on it — sees real money.
///
/// Adapters that *do* supply their own cost (the proxy adapter, which passes
/// through provider-billed amounts) keep precedence: a non-zero [`Cost`] is
/// left untouched.
///
/// Returns `true` if the message was repriced, `false` if it was left as-is
/// (adapter already priced it, or the model has no catalog pricing).
///
/// # Example
/// ```rust
/// use swink_agent::{AssistantMessage, Cost, StopReason, Usage, price_assistant_message};
///
/// let mut message = AssistantMessage {
///     content: vec![],
///     provider: "anthropic".to_string(),
///     model_id: "claude-sonnet-4-6".to_string(),
///     usage: Usage {
///         input: 1_000_000,
///         ..Usage::default()
///     },
///     cost: Cost::default(),
///     stop_reason: StopReason::Stop,
///     error_message: None,
///     error_kind: None,
///     timestamp: 0,
///     cache_hint: None,
/// };
///
/// assert!(price_assistant_message(&mut message));
/// assert!((message.cost.total - 3.0).abs() < 1e-9);
/// ```
pub fn price_assistant_message(message: &mut AssistantMessage) -> bool {
    if !message.cost.is_zero() {
        return false;
    }
    let priced = calculate_cost(&message.model_id, &message.usage);
    if priced.is_zero() {
        return false;
    }
    message.cost = priced;
    true
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

        // The compiled catalog (string statuses only) must still parse and
        // contain no deprecated entries today.
        let compiled = model_catalog();
        for provider in &compiled.providers {
            for preset in &provider.presets {
                assert!(
                    !preset
                        .status
                        .as_ref()
                        .is_some_and(PresetStatus::is_deprecated),
                    "unexpected deprecated preset {}.{}",
                    provider.key,
                    preset.id
                );
            }
        }
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
}
