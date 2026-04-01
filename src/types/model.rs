use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ─── Model Capabilities ─────────────────────────────────────────────────────

/// Per-model capability flags and limits.
///
/// Populated from the model catalog or set manually. The agent loop can
/// inspect these before enabling provider-specific features (e.g. skip
/// thinking blocks when `supports_thinking` is false).
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ModelCapabilities {
    /// Whether the model supports extended-thinking / chain-of-thought blocks.
    pub supports_thinking: bool,
    /// Whether the model accepts image content blocks.
    pub supports_vision: bool,
    /// Whether the model can invoke tools.
    pub supports_tool_use: bool,
    /// Whether the model supports streaming responses.
    pub supports_streaming: bool,
    /// Whether the model supports structured (JSON schema) output.
    pub supports_structured_output: bool,
    /// Maximum input context window in tokens, if known.
    pub max_context_window: Option<u64>,
    /// Maximum output tokens per response, if known.
    pub max_output_tokens: Option<u64>,
}

impl ModelCapabilities {
    /// Create capabilities with all flags set to false and no limits.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    #[must_use]
    pub const fn with_thinking(mut self, val: bool) -> Self {
        self.supports_thinking = val;
        self
    }

    #[must_use]
    pub const fn with_vision(mut self, val: bool) -> Self {
        self.supports_vision = val;
        self
    }

    #[must_use]
    pub const fn with_tool_use(mut self, val: bool) -> Self {
        self.supports_tool_use = val;
        self
    }

    #[must_use]
    pub const fn with_streaming(mut self, val: bool) -> Self {
        self.supports_streaming = val;
        self
    }

    #[must_use]
    pub const fn with_structured_output(mut self, val: bool) -> Self {
        self.supports_structured_output = val;
        self
    }

    #[must_use]
    pub const fn with_max_context_window(mut self, tokens: u64) -> Self {
        self.max_context_window = Some(tokens);
        self
    }

    #[must_use]
    pub const fn with_max_output_tokens(mut self, tokens: u64) -> Self {
        self.max_output_tokens = Some(tokens);
        self
    }
}

// ─── Model Specification ────────────────────────────────────────────────────

/// Reasoning depth for models that support configurable thinking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    #[default]
    Off,
    Minimal,
    Low,
    Medium,
    High,
    ExtraHigh,
}

/// Optional per-level token budget overrides for providers that support
/// token-based reasoning control.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingBudgets {
    pub budgets: HashMap<ThinkingLevel, u64>,
}

impl ThinkingBudgets {
    /// Create a new `ThinkingBudgets` from a map.
    #[must_use]
    pub const fn new(budgets: HashMap<ThinkingLevel, u64>) -> Self {
        Self { budgets }
    }

    /// Look up the token budget for a given thinking level.
    #[must_use]
    pub fn get(&self, level: &ThinkingLevel) -> Option<u64> {
        self.budgets.get(level).copied()
    }
}

/// Identifies the target model for a request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::derive_partial_eq_without_eq)]
pub struct ModelSpec {
    pub provider: String,
    pub model_id: String,
    pub thinking_level: ThinkingLevel,
    pub thinking_budgets: Option<ThinkingBudgets>,
    /// Provider-specific configuration (thinking, parameters, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_config: Option<serde_json::Value>,
    /// Per-model capability flags and limits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ModelCapabilities>,
}

impl ModelSpec {
    /// Create a new `ModelSpec` with thinking disabled and no budgets.
    #[must_use]
    pub fn new(provider: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model_id: model_id.into(),
            thinking_level: ThinkingLevel::Off,
            thinking_budgets: None,
            provider_config: None,
            capabilities: None,
        }
    }

    #[must_use]
    pub const fn with_thinking_level(mut self, level: ThinkingLevel) -> Self {
        self.thinking_level = level;
        self
    }

    #[must_use]
    pub fn with_thinking_budgets(mut self, budgets: ThinkingBudgets) -> Self {
        self.thinking_budgets = Some(budgets);
        self
    }

    #[must_use]
    pub fn with_provider_config(mut self, config: serde_json::Value) -> Self {
        self.provider_config = Some(config);
        self
    }

    #[must_use]
    pub const fn with_capabilities(mut self, capabilities: ModelCapabilities) -> Self {
        self.capabilities = Some(capabilities);
        self
    }

    /// Returns the model capabilities, or a default (all-false) set if none
    /// were provided.
    #[must_use]
    pub fn capabilities(&self) -> ModelCapabilities {
        self.capabilities.clone().unwrap_or_default()
    }

    /// Get a typed provider config, deserializing from the stored JSON.
    #[must_use]
    pub fn provider_config_as<T: serde::de::DeserializeOwned>(&self) -> Option<T> {
        self.provider_config
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }
}
