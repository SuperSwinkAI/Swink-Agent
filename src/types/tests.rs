//! Tests for `mod`.
#![cfg(test)]

use super::*;

#[test]
fn content_block_extension_serde_roundtrip() {
    let block = ContentBlock::Extension {
        type_name: "audio_clip".into(),
        data: serde_json::json!({"duration_ms": 1500, "codec": "opus"}),
    };
    let json = serde_json::to_string(&block).unwrap();
    let parsed: ContentBlock = serde_json::from_str(&json).unwrap();
    assert_eq!(block, parsed);
}

#[test]
fn extract_text_ignores_extension() {
    let blocks = vec![
        ContentBlock::Text {
            text: "hello ".into(),
        },
        ContentBlock::Extension {
            type_name: "image".into(),
            data: serde_json::json!({"url": "https://example.com/img.png"}),
        },
        ContentBlock::Text {
            text: "world".into(),
        },
    ];
    assert_eq!(ContentBlock::extract_text(&blocks), "hello world");
}

#[test]
fn usage_extra_add_merges_maps() {
    let a = Usage {
        input: 10,
        output: 5,
        extra: HashMap::from([
            ("reasoning_tokens".into(), 100),
            ("search_tokens".into(), 50),
        ]),
        ..Default::default()
    };
    let b = Usage {
        input: 20,
        output: 10,
        extra: HashMap::from([("reasoning_tokens".into(), 200), ("new_metric".into(), 30)]),
        ..Default::default()
    };
    let c = a + b;
    assert_eq!(c.input, 30);
    assert_eq!(c.output, 15);
    assert_eq!(c.extra["reasoning_tokens"], 300);
    assert_eq!(c.extra["search_tokens"], 50);
    assert_eq!(c.extra["new_metric"], 30);
}

#[test]
fn cost_extra_add_merges_maps() {
    let a = Cost {
        input: 0.01,
        output: 0.02,
        extra: HashMap::from([("reasoning_cost".into(), 0.05)]),
        ..Default::default()
    };
    let b = Cost {
        input: 0.03,
        output: 0.04,
        extra: HashMap::from([
            ("reasoning_cost".into(), 0.10),
            ("search_cost".into(), 0.02),
        ]),
        ..Default::default()
    };
    let c = a + b;
    assert!((c.input - 0.04).abs() < f64::EPSILON);
    assert!((c.output - 0.06).abs() < f64::EPSILON);
    assert!((c.extra["reasoning_cost"] - 0.15).abs() < f64::EPSILON);
    assert!((c.extra["search_cost"] - 0.02).abs() < f64::EPSILON);
}

#[test]
fn model_spec_with_provider_config() {
    let config = serde_json::json!({
        "temperature": 0.7,
        "top_p": 0.9,
    });

    let spec = ModelSpec::new("anthropic", "claude-3").with_provider_config(config.clone());

    assert_eq!(spec.provider_config, Some(config));
    assert_eq!(spec.provider, "anthropic");
    assert_eq!(spec.model_id, "claude-3");
}

#[test]
fn provider_config_as_typed() {
    #[derive(Debug, Deserialize, PartialEq)]
    struct MyConfig {
        temperature: f64,
        max_tokens: u32,
    }

    let spec = ModelSpec::new("openai", "gpt-4").with_provider_config(serde_json::json!({
        "temperature": 0.5,
        "max_tokens": 1024,
    }));

    let config: Option<MyConfig> = spec.provider_config_as();
    assert_eq!(
        config,
        Some(MyConfig {
            temperature: 0.5,
            max_tokens: 1024,
        })
    );

    // None when no provider_config is set.
    let spec_none = ModelSpec::new("openai", "gpt-4");
    let config_none: Option<MyConfig> = spec_none.provider_config_as();
    assert!(config_none.is_none());
}

#[test]
fn model_capabilities_builder_chain() {
    let caps = ModelCapabilities::none()
        .with_thinking(true)
        .with_vision(true)
        .with_tool_use(true)
        .with_streaming(true)
        .with_structured_output(true)
        .with_max_context_window(200_000)
        .with_max_output_tokens(16384);

    assert!(caps.supports_thinking);
    assert!(caps.supports_vision);
    assert!(caps.supports_tool_use);
    assert!(caps.supports_streaming);
    assert!(caps.supports_structured_output);
    assert_eq!(caps.max_context_window, Some(200_000));
    assert_eq!(caps.max_output_tokens, Some(16384));
}

#[test]
fn model_capabilities_serde_roundtrip() {
    let caps = ModelCapabilities::none()
        .with_thinking(true)
        .with_tool_use(true)
        .with_max_context_window(128_000);
    let json = serde_json::to_string(&caps).unwrap();
    let parsed: ModelCapabilities = serde_json::from_str(&json).unwrap();
    assert_eq!(caps, parsed);
}

#[test]
fn model_spec_with_capabilities() {
    let caps = ModelCapabilities::none()
        .with_thinking(true)
        .with_streaming(true);
    let spec = ModelSpec::new("test", "model-1").with_capabilities(caps.clone());
    assert_eq!(spec.capabilities, Some(caps.clone()));
    assert_eq!(spec.capabilities(), caps);
}

#[test]
fn model_spec_capabilities_defaults_when_none() {
    let spec = ModelSpec::new("test", "model-1");
    assert!(spec.capabilities.is_none());
    let caps = spec.capabilities();
    assert!(!caps.supports_thinking);
    assert_eq!(caps.max_context_window, None);
}

// ─── Custom Message Serialization ────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct MockNotification {
    title: String,
    body: String,
}

impl CustomMessage for MockNotification {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn type_name(&self) -> Option<&str> {
        Some("mock_notification")
    }

    fn to_json(&self) -> Option<serde_json::Value> {
        serde_json::to_value(self).ok()
    }
}

#[test]
fn custom_message_serialize_roundtrip() {
    let msg = MockNotification {
        title: "Hello".into(),
        body: "World".into(),
    };

    let envelope = serialize_custom_message(&msg).expect("serialization supported");
    assert_eq!(envelope["type"], "mock_notification");
    assert_eq!(envelope["data"]["title"], "Hello");

    let mut registry = CustomMessageRegistry::new();
    registry.register_type::<MockNotification>("mock_notification");

    let restored = deserialize_custom_message(&registry, &envelope).unwrap();
    let downcasted = restored
        .as_any()
        .downcast_ref::<MockNotification>()
        .unwrap();
    assert_eq!(downcasted, &msg);
}

#[test]
fn custom_message_default_returns_none() {
    #[derive(Debug)]
    struct Bare;
    impl CustomMessage for Bare {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }
    let bare = Bare;
    assert!(bare.type_name().is_none());
    assert!(bare.to_json().is_none());
    assert!(serialize_custom_message(&bare).is_none());
}

#[test]
fn registry_unknown_type_returns_error() {
    let registry = CustomMessageRegistry::new();
    let envelope = serde_json::json!({"type": "unknown", "data": {}});
    let result = deserialize_custom_message(&registry, &envelope);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("no deserializer registered"));
}

#[test]
fn registry_contains_check() {
    let mut registry = CustomMessageRegistry::new();
    assert!(!registry.has_type_name("mock_notification"));
    registry.register_type::<MockNotification>("mock_notification");
    assert!(registry.has_type_name("mock_notification"));
}

#[test]
fn assistant_text_extracts_last_assistant_message() {
    let result = AgentResult {
        messages: vec![
            AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "hi".to_string(),
                }],
                timestamp: 0,
                cache_hint: None,
            })),
            AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
                content: vec![ContentBlock::Text {
                    text: "first".to_string(),
                }],
                provider: "test".to_string(),
                model_id: "m".to_string(),
                usage: Usage::default(),
                cost: Cost::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                error_kind: None,
                timestamp: 0,
                cache_hint: None,
            })),
            AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
                content: vec![ContentBlock::Text {
                    text: "second".to_string(),
                }],
                provider: "test".to_string(),
                model_id: "m".to_string(),
                usage: Usage::default(),
                cost: Cost::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                error_kind: None,
                timestamp: 0,
                cache_hint: None,
            })),
        ],
        stop_reason: StopReason::Stop,
        usage: Usage::default(),
        cost: Cost::default(),
        error: None,
        transfer_signal: None,
    };
    assert_eq!(result.assistant_text(), "second");
}

#[test]
fn assistant_text_returns_empty_when_no_assistant() {
    let result = AgentResult {
        messages: vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "hi".to_string(),
            }],
            timestamp: 0,
            cache_hint: None,
        }))],
        stop_reason: StopReason::Stop,
        usage: Usage::default(),
        cost: Cost::default(),
        error: None,
        transfer_signal: None,
    };
    assert_eq!(result.assistant_text(), "");
}

#[test]
fn assistant_text_returns_empty_when_no_messages() {
    let result = AgentResult {
        messages: vec![],
        stop_reason: StopReason::Stop,
        usage: Usage::default(),
        cost: Cost::default(),
        error: None,
        transfer_signal: None,
    };
    assert_eq!(result.assistant_text(), "");
}

#[test]
fn deserialize_custom_message_missing_fields() {
    let registry = CustomMessageRegistry::new();

    let no_type = serde_json::json!({"data": {}});
    assert!(
        deserialize_custom_message(&registry, &no_type)
            .unwrap_err()
            .contains("missing 'type'")
    );

    let no_data = serde_json::json!({"type": "foo"});
    assert!(
        deserialize_custom_message(&registry, &no_data)
            .unwrap_err()
            .contains("missing 'data'")
    );
}
