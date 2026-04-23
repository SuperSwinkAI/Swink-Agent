//! Serializable agent configuration.
//!
//! [`AgentConfig`] captures the subset of [`AgentOptions`](crate::AgentOptions)
//! that can be round-tripped through serde. Trait objects (tools, transformers,
//! policies, callbacks) are represented by name so they can be re-registered
//! after deserialization.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::stream::StreamTransport;
use crate::tool::ApprovalMode;
use crate::types::ModelSpec;

// ─── RetryConfig ─────────────────────────────────────────────────────────────

/// Serializable representation of [`DefaultRetryStrategy`](crate::DefaultRetryStrategy) parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Maximum number of attempts (including the initial call).
    pub max_attempts: u32,
    /// Base delay in milliseconds before the first retry.
    pub base_delay_ms: u64,
    /// Maximum delay cap in milliseconds.
    pub max_delay_ms: u64,
    /// Exponential multiplier per attempt.
    pub multiplier: f64,
    /// Whether jitter is applied to delays.
    pub jitter: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        let default = crate::retry::DefaultRetryStrategy::default();
        Self {
            max_attempts: default.max_attempts,
            base_delay_ms: default
                .base_delay
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
            max_delay_ms: default.max_delay.as_millis().try_into().unwrap_or(u64::MAX),
            multiplier: default.multiplier,
            jitter: default.jitter,
        }
    }
}

impl From<&crate::retry::DefaultRetryStrategy> for RetryConfig {
    fn from(s: &crate::retry::DefaultRetryStrategy) -> Self {
        Self {
            max_attempts: s.max_attempts,
            base_delay_ms: s.base_delay.as_millis().try_into().unwrap_or(u64::MAX),
            max_delay_ms: s.max_delay.as_millis().try_into().unwrap_or(u64::MAX),
            multiplier: s.multiplier,
            jitter: s.jitter,
        }
    }
}

impl RetryConfig {
    /// Convert back to a [`DefaultRetryStrategy`](crate::DefaultRetryStrategy).
    #[must_use]
    pub const fn to_retry_strategy(&self) -> crate::retry::DefaultRetryStrategy {
        crate::retry::DefaultRetryStrategy {
            max_attempts: self.max_attempts,
            base_delay: Duration::from_millis(self.base_delay_ms),
            max_delay: Duration::from_millis(self.max_delay_ms),
            multiplier: self.multiplier,
            jitter: self.jitter,
        }
    }
}

// ─── StreamOptionsConfig ─────────────────────────────────────────────────────

/// Serializable representation of [`StreamOptions`](crate::StreamOptions).
///
/// The `api_key` field is intentionally omitted — secrets should not be
/// persisted in config snapshots.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StreamOptionsConfig {
    /// Sampling temperature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Output token limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    /// Provider-side session identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Preferred transport protocol.
    #[serde(default)]
    pub transport: StreamTransport,
}

impl From<&crate::stream::StreamOptions> for StreamOptionsConfig {
    fn from(opts: &crate::stream::StreamOptions) -> Self {
        Self {
            temperature: opts.temperature,
            max_tokens: opts.max_tokens,
            session_id: opts.session_id.clone(),
            transport: opts.transport,
        }
    }
}

impl StreamOptionsConfig {
    /// Convert back to [`StreamOptions`](crate::StreamOptions), leaving `api_key` as `None`.
    #[must_use]
    pub fn to_stream_options(&self) -> crate::stream::StreamOptions {
        crate::stream::StreamOptions {
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            session_id: self.session_id.clone(),
            api_key: None,
            transport: self.transport,
            cache_strategy: crate::stream::CacheStrategy::default(),
            on_raw_payload: None,
        }
    }
}

// ─── SteeringMode / FollowUpMode serde wrappers ─────────────────────────────

/// Serializable mirror of [`SteeringMode`](crate::SteeringMode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SteeringModeConfig {
    All,
    #[default]
    OneAtATime,
}

impl From<crate::agent::SteeringMode> for SteeringModeConfig {
    fn from(m: crate::agent::SteeringMode) -> Self {
        match m {
            crate::agent::SteeringMode::All => Self::All,
            crate::agent::SteeringMode::OneAtATime => Self::OneAtATime,
        }
    }
}

impl From<SteeringModeConfig> for crate::agent::SteeringMode {
    fn from(m: SteeringModeConfig) -> Self {
        match m {
            SteeringModeConfig::All => Self::All,
            SteeringModeConfig::OneAtATime => Self::OneAtATime,
        }
    }
}

/// Serializable mirror of [`FollowUpMode`](crate::FollowUpMode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FollowUpModeConfig {
    All,
    #[default]
    OneAtATime,
}

impl From<crate::agent::FollowUpMode> for FollowUpModeConfig {
    fn from(m: crate::agent::FollowUpMode) -> Self {
        match m {
            crate::agent::FollowUpMode::All => Self::All,
            crate::agent::FollowUpMode::OneAtATime => Self::OneAtATime,
        }
    }
}

impl From<FollowUpModeConfig> for crate::agent::FollowUpMode {
    fn from(m: FollowUpModeConfig) -> Self {
        match m {
            FollowUpModeConfig::All => Self::All,
            FollowUpModeConfig::OneAtATime => Self::OneAtATime,
        }
    }
}

/// Serializable mirror of [`ApprovalMode`](crate::ApprovalMode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalModeConfig {
    #[default]
    Enabled,
    Smart,
    Bypassed,
}

// ─── CacheConfigData ────────────────────────────────────────────────────────

/// Serializable representation of [`CacheConfig`](crate::context_cache::CacheConfig).
///
/// Duration is stored as milliseconds for serde-friendliness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfigData {
    /// Time-to-live in milliseconds.
    pub ttl_ms: u64,
    /// Minimum token count for the cached prefix.
    pub min_tokens: usize,
    /// Number of turns between cache refreshes.
    pub cache_intervals: usize,
}

impl From<&crate::context_cache::CacheConfig> for CacheConfigData {
    fn from(c: &crate::context_cache::CacheConfig) -> Self {
        Self {
            ttl_ms: c.ttl.as_millis().try_into().unwrap_or(u64::MAX),
            min_tokens: c.min_tokens,
            cache_intervals: c.cache_intervals,
        }
    }
}

impl CacheConfigData {
    /// Convert back to a [`CacheConfig`](crate::context_cache::CacheConfig).
    #[must_use]
    pub const fn to_cache_config(&self) -> crate::context_cache::CacheConfig {
        crate::context_cache::CacheConfig::new(
            std::time::Duration::from_millis(self.ttl_ms),
            self.min_tokens,
            self.cache_intervals,
        )
    }
}

impl From<ApprovalMode> for ApprovalModeConfig {
    fn from(m: ApprovalMode) -> Self {
        match m {
            ApprovalMode::Enabled => Self::Enabled,
            ApprovalMode::Smart => Self::Smart,
            ApprovalMode::Bypassed => Self::Bypassed,
        }
    }
}

impl From<ApprovalModeConfig> for ApprovalMode {
    fn from(m: ApprovalModeConfig) -> Self {
        match m {
            ApprovalModeConfig::Enabled => Self::Enabled,
            ApprovalModeConfig::Smart => Self::Smart,
            ApprovalModeConfig::Bypassed => Self::Bypassed,
        }
    }
}

// ─── AgentConfig ─────────────────────────────────────────────────────────────

/// A fully serializable snapshot of agent configuration.
///
/// Captures the subset of [`AgentOptions`](crate::AgentOptions) fields that can
/// survive a serde round-trip. Trait objects (tools, stream functions,
/// transformers, policies, callbacks) **cannot** be serialized and must be
/// re-registered by the consumer after deserialization.
///
/// # What round-trips faithfully
///
/// `system_prompt`, `model`, `retry`, `stream_options`, `steering_mode`,
/// `follow_up_mode`, `structured_output_max_retries`, `approval_mode`,
/// `plan_mode_addendum`, and `cache_config` are all restored by
/// [`into_agent_options()`](Self::into_agent_options).
///
/// # What does NOT round-trip
///
/// - **`tool_names`** — stored for informational use only (e.g., re-registering
///   tools by name). The consumer must supply the actual tool implementations.
/// - **`extra`** — application-level metadata that has no corresponding
///   `AgentOptions` field. Survives serde but is not fed back into the agent.
/// - **Trait objects** — `stream_fn`, `convert_to_llm`, `transform_context`,
///   `approve_tool`, policies, event forwarders, etc. must be re-attached.
///
/// # Example
///
/// ```ignore
/// // Save
/// let config = agent.options().to_config();
/// let json = serde_json::to_string(&config)?;
///
/// // Restore
/// let config: AgentConfig = serde_json::from_str(&json)?;
/// let opts = AgentOptions::from_config(config, stream_fn, convert_to_llm)
///     .with_tools(re_register_tools(&config.tool_names));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// System prompt sent to the LLM.
    pub system_prompt: String,

    /// Model specification (provider, model ID, thinking level, etc.).
    pub model: ModelSpec,

    /// Names of registered tools (routing keys from [`AgentTool::name()`](crate::AgentTool::name)).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_names: Vec<String>,

    /// Retry strategy parameters.
    #[serde(default)]
    pub retry: RetryConfig,

    /// Per-call stream options (temperature, max tokens, transport).
    #[serde(default)]
    pub stream_options: StreamOptionsConfig,

    /// Steering queue drain mode.
    #[serde(default)]
    pub steering_mode: SteeringModeConfig,

    /// Follow-up queue drain mode.
    #[serde(default)]
    pub follow_up_mode: FollowUpModeConfig,

    /// Max retries for structured output validation.
    #[serde(default = "default_structured_output_max_retries")]
    pub structured_output_max_retries: usize,

    /// Approval mode for the tool gate.
    #[serde(default)]
    pub approval_mode: ApprovalModeConfig,

    /// Optional plan mode addendum appended to the system prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_mode_addendum: Option<String>,

    /// Optional context caching configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_config: Option<CacheConfigData>,

    /// Arbitrary extension data for application-specific config.
    ///
    /// This field survives serialization but is **not** restored into
    /// [`AgentOptions`](crate::AgentOptions) — it has no corresponding field
    /// there. Use it to store application-level metadata alongside the config.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub extra: serde_json::Value,
}

const fn default_structured_output_max_retries() -> usize {
    3
}

impl AgentConfig {
    /// Restore an [`AgentOptions`](crate::AgentOptions) builder from this config.
    ///
    /// The caller must supply the required non-serializable arguments
    /// (`stream_fn` and `convert_to_llm`) and then re-attach any trait objects
    /// (tools, transformers, policies) via the builder methods.
    #[must_use]
    pub fn into_agent_options(
        self,
        stream_fn: std::sync::Arc<dyn crate::stream::StreamFn>,
        convert_to_llm: impl Fn(&crate::types::AgentMessage) -> Option<crate::types::LlmMessage>
        + Send
        + Sync
        + 'static,
    ) -> crate::agent::AgentOptions {
        let mut opts = crate::agent::AgentOptions::new(
            self.system_prompt,
            self.model,
            stream_fn,
            convert_to_llm,
        );

        opts.retry_strategy = Box::new(self.retry.to_retry_strategy());
        opts.stream_options = self.stream_options.to_stream_options();
        opts.steering_mode = self.steering_mode.into();
        opts.follow_up_mode = self.follow_up_mode.into();
        opts.structured_output_max_retries = self.structured_output_max_retries;
        opts.approval_mode = self.approval_mode.into();
        opts.plan_mode_addendum = self.plan_mode_addendum;
        opts.cache_config = self.cache_config.map(|c| c.to_cache_config());

        // Clear the default transform_context — the caller may want to re-attach
        // their own, and `from_config` should not silently override.
        opts.transform_context = None;

        opts
    }
}

// ─── AgentOptions::to_config / from_config ───────────────────────────────────

impl crate::agent::AgentOptions {
    /// Extract a serializable [`AgentConfig`] from these options.
    ///
    /// Tool implementations are represented by name only. Trait objects
    /// (transformers, policies, callbacks) are omitted — their presence must
    /// be restored by the consumer after deserialization.
    #[must_use]
    pub fn to_config(&self) -> AgentConfig {
        let tool_names: Vec<String> = self.tools.iter().map(|t| t.name().to_string()).collect();

        // Attempt to extract retry params from a DefaultRetryStrategy. If the
        // caller used a custom RetryStrategy we fall back to defaults.
        let retry = downcast_retry_config(&*self.retry_strategy);

        AgentConfig {
            system_prompt: self.system_prompt.clone(),
            model: self.model.clone(),
            tool_names,
            retry,
            stream_options: StreamOptionsConfig::from(&self.stream_options),
            steering_mode: self.steering_mode.into(),
            follow_up_mode: self.follow_up_mode.into(),
            structured_output_max_retries: self.structured_output_max_retries,
            approval_mode: self.approval_mode.into(),
            plan_mode_addendum: self.plan_mode_addendum.clone(),
            cache_config: self.cache_config.as_ref().map(CacheConfigData::from),
            extra: serde_json::Value::Null,
        }
    }

    /// Construct `AgentOptions` from a deserialized [`AgentConfig`].
    ///
    /// Equivalent to [`AgentConfig::into_agent_options`] — provided here for
    /// discoverability.
    #[must_use]
    pub fn from_config(
        config: AgentConfig,
        stream_fn: std::sync::Arc<dyn crate::stream::StreamFn>,
        convert_to_llm: impl Fn(&crate::types::AgentMessage) -> Option<crate::types::LlmMessage>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        config.into_agent_options(stream_fn, convert_to_llm)
    }
}

/// Try to downcast the retry strategy to `DefaultRetryStrategy` and extract its
/// parameters. Falls back to `RetryConfig::default()` for custom strategies.
fn downcast_retry_config(strategy: &dyn crate::retry::RetryStrategy) -> RetryConfig {
    strategy
        .as_any()
        .downcast_ref::<crate::retry::DefaultRetryStrategy>()
        .map_or_else(RetryConfig::default, RetryConfig::from)
}

// ─── Send + Sync assertions ─────────────────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AgentConfig>();
    assert_send_sync::<RetryConfig>();
    assert_send_sync::<StreamOptionsConfig>();
    assert_send_sync::<SteeringModeConfig>();
    assert_send_sync::<FollowUpModeConfig>();
    assert_send_sync::<ApprovalModeConfig>();
};

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ThinkingLevel;

    #[test]
    fn retry_config_roundtrip() {
        let config = RetryConfig {
            max_attempts: 5,
            base_delay_ms: 2000,
            max_delay_ms: 120_000,
            multiplier: 3.0,
            jitter: false,
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: RetryConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.max_attempts, 5);
        assert_eq!(restored.base_delay_ms, 2000);
        assert_eq!(restored.max_delay_ms, 120_000);
        assert!((restored.multiplier - 3.0).abs() < f64::EPSILON);
        assert!(!restored.jitter);
    }

    #[test]
    fn retry_config_to_strategy_and_back() {
        let config = RetryConfig {
            max_attempts: 4,
            base_delay_ms: 500,
            max_delay_ms: 30_000,
            multiplier: 1.5,
            jitter: true,
        };
        let strategy = config.to_retry_strategy();
        assert_eq!(strategy.max_attempts, 4);
        assert_eq!(strategy.base_delay, Duration::from_millis(500));
        assert_eq!(strategy.max_delay, Duration::from_secs(30));
        assert!((strategy.multiplier - 1.5).abs() < f64::EPSILON);
        assert!(strategy.jitter);

        let back = RetryConfig::from(&strategy);
        assert_eq!(back.max_attempts, 4);
        assert_eq!(back.base_delay_ms, 500);
    }

    #[test]
    fn stream_options_config_roundtrip() {
        let config = StreamOptionsConfig {
            temperature: Some(0.7),
            max_tokens: Some(4096),
            session_id: Some("sess-123".into()),
            transport: StreamTransport::Sse,
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: StreamOptionsConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.temperature, Some(0.7));
        assert_eq!(restored.max_tokens, Some(4096));
        assert_eq!(restored.session_id.as_deref(), Some("sess-123"));
    }

    #[test]
    fn stream_options_config_omits_api_key() {
        let opts = crate::stream::StreamOptions {
            temperature: Some(0.5),
            max_tokens: None,
            session_id: None,
            api_key: Some("secret-key".into()),
            transport: StreamTransport::Sse,
            cache_strategy: crate::stream::CacheStrategy::default(),
            on_raw_payload: None,
        };
        let config = StreamOptionsConfig::from(&opts);
        let json = serde_json::to_string(&config).unwrap();
        assert!(!json.contains("secret-key"));

        let restored_opts = config.to_stream_options();
        assert!(restored_opts.api_key.is_none());
        assert_eq!(restored_opts.temperature, Some(0.5));
    }

    #[test]
    fn agent_config_serde_roundtrip() {
        let config = AgentConfig {
            system_prompt: "Be helpful.".into(),
            model: ModelSpec::new("anthropic", "claude-sonnet")
                .with_thinking_level(ThinkingLevel::Medium),
            tool_names: vec!["bash".into(), "read_file".into()],
            retry: RetryConfig {
                max_attempts: 5,
                base_delay_ms: 1000,
                max_delay_ms: 60_000,
                multiplier: 2.0,
                jitter: true,
            },
            stream_options: StreamOptionsConfig {
                temperature: Some(0.7),
                max_tokens: Some(8192),
                session_id: None,
                transport: StreamTransport::Sse,
            },
            steering_mode: SteeringModeConfig::OneAtATime,
            follow_up_mode: FollowUpModeConfig::All,
            structured_output_max_retries: 5,
            approval_mode: ApprovalModeConfig::Smart,
            plan_mode_addendum: Some("Custom plan instructions.".into()),
            cache_config: Some(CacheConfigData {
                ttl_ms: 300_000,
                min_tokens: 1024,
                cache_intervals: 4,
            }),
            extra: serde_json::json!({"custom_key": "custom_value"}),
        };

        let json = serde_json::to_string_pretty(&config).unwrap();
        let restored: AgentConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.system_prompt, "Be helpful.");
        assert_eq!(restored.model.provider, "anthropic");
        assert_eq!(restored.model.model_id, "claude-sonnet");
        assert_eq!(restored.model.thinking_level, ThinkingLevel::Medium);
        assert_eq!(restored.tool_names, vec!["bash", "read_file"]);
        assert_eq!(restored.retry.max_attempts, 5);
        assert_eq!(restored.stream_options.temperature, Some(0.7));
        assert_eq!(restored.stream_options.max_tokens, Some(8192));
        assert_eq!(restored.steering_mode, SteeringModeConfig::OneAtATime);
        assert_eq!(restored.follow_up_mode, FollowUpModeConfig::All);
        assert_eq!(restored.structured_output_max_retries, 5);
        assert_eq!(restored.approval_mode, ApprovalModeConfig::Smart);
        assert_eq!(
            restored.plan_mode_addendum.as_deref(),
            Some("Custom plan instructions.")
        );
        let cc = restored.cache_config.unwrap();
        assert_eq!(cc.ttl_ms, 300_000);
        assert_eq!(cc.min_tokens, 1024);
        assert_eq!(cc.cache_intervals, 4);
        assert_eq!(restored.extra["custom_key"], "custom_value");
    }

    #[test]
    fn agent_config_minimal_json_deserializes() {
        // Only required fields; everything else falls back to defaults.
        let json = r#"{
            "system_prompt": "Hello",
            "model": {
                "provider": "openai",
                "model_id": "gpt-4",
                "thinking_level": "off"
            }
        }"#;

        let config: AgentConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.system_prompt, "Hello");
        assert_eq!(config.model.provider, "openai");
        assert!(config.tool_names.is_empty());
        assert_eq!(config.retry.max_attempts, 3); // default
        assert_eq!(config.structured_output_max_retries, 3); // default
    }

    #[test]
    fn old_json_with_removed_fields_still_deserializes() {
        // Configs saved before these fields were removed should still load.
        let json = r#"{
            "system_prompt": "Hello",
            "model": { "provider": "openai", "model_id": "gpt-4", "thinking_level": "off" },
            "available_models": [{ "provider": "openai", "model_id": "gpt-4o", "thinking_level": "off" }],
            "fallback_models": [{ "provider": "openai", "model_id": "gpt-4o-mini", "thinking_level": "off" }],
            "budget_guard": { "max_cost": 10.0, "max_tokens": 100000 }
        }"#;
        let config: AgentConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.system_prompt, "Hello");
        assert_eq!(config.model.provider, "openai");
    }

    #[test]
    #[cfg(feature = "testkit")]
    fn config_round_trip_only_contains_restorable_fields() {
        // Every field in AgentConfig (except `extra` and `tool_names`, which
        // are documented as metadata-only) must be faithfully restored by
        // into_agent_options(). This test guards against adding fields
        // that serialize but silently drop on restore.
        let config = AgentConfig {
            system_prompt: "test".into(),
            model: ModelSpec::new("anthropic", "claude-sonnet"),
            tool_names: vec!["bash".into()],
            retry: RetryConfig {
                max_attempts: 7,
                base_delay_ms: 500,
                max_delay_ms: 10_000,
                multiplier: 1.5,
                jitter: false,
            },
            stream_options: StreamOptionsConfig {
                temperature: Some(0.3),
                max_tokens: Some(2048),
                session_id: Some("s1".into()),
                transport: StreamTransport::Sse,
            },
            steering_mode: SteeringModeConfig::All,
            follow_up_mode: FollowUpModeConfig::All,
            structured_output_max_retries: 10,
            approval_mode: ApprovalModeConfig::Bypassed,
            plan_mode_addendum: Some("Plan mode text.".into()),
            cache_config: Some(CacheConfigData {
                ttl_ms: 60_000,
                min_tokens: 512,
                cache_intervals: 3,
            }),
            extra: serde_json::json!({"k": "v"}),
        };

        let stream_fn: std::sync::Arc<dyn crate::stream::StreamFn> =
            std::sync::Arc::new(crate::testing::MockStreamFn::new(vec![]));
        let opts = config
            .clone()
            .into_agent_options(stream_fn, crate::agent::default_convert);

        assert_eq!(opts.system_prompt, config.system_prompt);
        assert_eq!(opts.model.provider, config.model.provider);
        assert_eq!(opts.model.model_id, config.model.model_id);
        assert_eq!(
            opts.stream_options.temperature,
            config.stream_options.temperature
        );
        assert_eq!(
            opts.stream_options.max_tokens,
            config.stream_options.max_tokens
        );
        assert_eq!(
            opts.structured_output_max_retries,
            config.structured_output_max_retries
        );
        assert!(matches!(
            opts.steering_mode,
            crate::agent::SteeringMode::All
        ));
        assert!(matches!(
            opts.follow_up_mode,
            crate::agent::FollowUpMode::All
        ));
        assert!(matches!(
            opts.approval_mode,
            crate::tool::ApprovalMode::Bypassed
        ));
        assert_eq!(opts.plan_mode_addendum.as_deref(), Some("Plan mode text."));
        let cc = opts.cache_config.unwrap();
        assert_eq!(cc.ttl.as_millis(), 60_000);
        assert_eq!(cc.min_tokens, 512);
        assert_eq!(cc.cache_intervals, 3);
    }

    #[test]
    fn approval_mode_config_roundtrip() {
        for (mode, expected) in [
            (ApprovalModeConfig::Enabled, "\"enabled\""),
            (ApprovalModeConfig::Smart, "\"smart\""),
            (ApprovalModeConfig::Bypassed, "\"bypassed\""),
        ] {
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(json, expected);
            let back: ApprovalModeConfig = serde_json::from_str(&json).unwrap();
            assert_eq!(back, mode);
        }
    }

    #[test]
    fn cache_config_data_roundtrip() {
        let data = CacheConfigData {
            ttl_ms: 120_000,
            min_tokens: 2048,
            cache_intervals: 5,
        };
        let cc = data.to_cache_config();
        assert_eq!(cc.ttl, Duration::from_mins(2));
        assert_eq!(cc.min_tokens, 2048);
        assert_eq!(cc.cache_intervals, 5);

        let back = CacheConfigData::from(&cc);
        assert_eq!(back.ttl_ms, 120_000);
        assert_eq!(back.min_tokens, 2048);
        assert_eq!(back.cache_intervals, 5);
    }

    #[test]
    #[cfg(feature = "testkit")]
    fn to_config_captures_plan_mode_and_cache() {
        let stream_fn: std::sync::Arc<dyn crate::stream::StreamFn> =
            std::sync::Arc::new(crate::testing::MockStreamFn::new(vec![]));
        let mut opts = crate::agent::AgentOptions::new(
            "test",
            crate::types::ModelSpec::new("anthropic", "claude-sonnet"),
            stream_fn,
            crate::agent::default_convert,
        );
        opts.plan_mode_addendum = Some("custom addendum".into());
        opts.cache_config = Some(crate::context_cache::CacheConfig::new(
            Duration::from_mins(5),
            1024,
            4,
        ));

        let config = opts.to_config();
        assert_eq!(
            config.plan_mode_addendum.as_deref(),
            Some("custom addendum")
        );
        let cc = config.cache_config.unwrap();
        assert_eq!(cc.ttl_ms, 300_000);
        assert_eq!(cc.min_tokens, 1024);
        assert_eq!(cc.cache_intervals, 4);
    }

    #[test]
    fn steering_follow_up_mode_conversions() {
        // SteeringMode round-trip
        let all: SteeringModeConfig = crate::agent::SteeringMode::All.into();
        assert_eq!(all, SteeringModeConfig::All);
        let back: crate::agent::SteeringMode = all.into();
        assert!(matches!(back, crate::agent::SteeringMode::All));

        // FollowUpMode round-trip
        let one: FollowUpModeConfig = crate::agent::FollowUpMode::OneAtATime.into();
        assert_eq!(one, FollowUpModeConfig::OneAtATime);
        let back: crate::agent::FollowUpMode = one.into();
        assert!(matches!(back, crate::agent::FollowUpMode::OneAtATime));
    }
}
