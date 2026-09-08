//! Tests for `config`.
#![cfg(test)]

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
fn retry_config_builders_set_every_field() {
    let config = RetryConfig::default()
        .with_max_attempts(7)
        .with_base_delay_ms(250)
        .with_max_delay_ms(9000)
        .with_multiplier(4.0)
        .with_jitter(false);
    assert_eq!(config.max_attempts, 7);
    assert_eq!(config.base_delay_ms, 250);
    assert_eq!(config.max_delay_ms, 9000);
    assert!((config.multiplier - 4.0).abs() < f64::EPSILON);
    assert!(!config.jitter);
}

#[test]
fn stream_options_config_builders_set_every_field() {
    let serving = crate::stream::ServingOptions {
        context_length: Some(2048),
        ..Default::default()
    };
    let config = StreamOptionsConfig::default()
        .with_temperature(0.3)
        .with_max_tokens(1024)
        .with_session_id("sess-9")
        .with_transport(StreamTransport::Sse)
        .with_serving(serving);
    assert_eq!(config.temperature, Some(0.3));
    assert_eq!(config.max_tokens, Some(1024));
    assert_eq!(config.session_id.as_deref(), Some("sess-9"));
    assert_eq!(config.transport, StreamTransport::Sse);
    assert_eq!(config.serving.context_length, Some(2048));
}

#[test]
fn stream_options_config_roundtrip() {
    let config = StreamOptionsConfig {
        temperature: Some(0.7),
        max_tokens: Some(4096),
        session_id: Some("sess-123".into()),
        transport: StreamTransport::Sse,
        serving: crate::stream::ServingOptions {
            context_length: Some(8192),
            keep_alive: Some("5m".into()),
            format: Some(crate::stream::ResponseFormat::Json),
            ..Default::default()
        },
    };
    let json = serde_json::to_string(&config).unwrap();
    let restored: StreamOptionsConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.temperature, Some(0.7));
    assert_eq!(restored.max_tokens, Some(4096));
    assert_eq!(restored.session_id.as_deref(), Some("sess-123"));
    assert_eq!(restored.serving.context_length, Some(8192));
    assert_eq!(restored.serving.keep_alive.as_deref(), Some("5m"));
    assert_eq!(
        restored.serving.format,
        Some(crate::stream::ResponseFormat::Json)
    );
}

/// `ResponseFormat::Schema` survives a config round-trip, and a `None`
/// format adds no key to the serialized form.
#[test]
fn stream_options_config_roundtrips_response_format_schema() {
    let schema = serde_json::json!({ "type": "object" });
    let config = StreamOptionsConfig {
        serving: crate::stream::ServingOptions {
            format: Some(crate::stream::ResponseFormat::Schema(schema.clone())),
            ..Default::default()
        },
        ..StreamOptionsConfig::default()
    };
    let json = serde_json::to_string(&config).unwrap();
    let restored: StreamOptionsConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(
        restored.serving.format,
        Some(crate::stream::ResponseFormat::Schema(schema))
    );

    let without = serde_json::to_string(&StreamOptionsConfig {
        serving: crate::stream::ServingOptions {
            context_length: Some(8192),
            ..Default::default()
        },
        ..StreamOptionsConfig::default()
    })
    .unwrap();
    assert!(
        !without.contains("format"),
        "`format: None` must not add config noise: {without}"
    );
}

#[test]
fn stream_options_config_omits_default_serving_options() {
    let json = serde_json::to_string(&StreamOptionsConfig::default()).unwrap();
    assert!(!json.contains("serving"));
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
        on_rate_limit: None,
        serving: crate::stream::ServingOptions::default(),
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
            serving: crate::stream::ServingOptions::default(),
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
            serving: crate::stream::ServingOptions::default(),
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
