use std::collections::HashMap;

use swink_agent::{
    AgentContext, AgentMessage, AgentResult, AssistantMessage, ContentBlock, Cost, CustomMessage,
    DowncastError, ImageSource, LlmMessage, ModelCapabilities, ModelSpec, StopReason,
    ThinkingBudgets, ThinkingLevel, ToolResultMessage, Usage, UserMessage,
};

// ═══════════════════════════════════════════════════════════════════════════
// Phase 3 — User Story 1: Message Types for Conversation History
// ═══════════════════════════════════════════════════════════════════════════

// T008
#[test]
fn user_message_construction_and_access() {
    let msg = UserMessage {
        content: vec![ContentBlock::Text {
            text: "hello world".into(),
        }],
        timestamp: 1710000000,
    };
    assert_eq!(msg.content.len(), 1);
    assert!(matches!(&msg.content[0], ContentBlock::Text { text } if text == "hello world"));
    assert_eq!(msg.timestamp, 1710000000);
}

// T009
#[test]
fn assistant_message_construction_and_access() {
    let msg = AssistantMessage {
        content: vec![ContentBlock::Text {
            text: "response".into(),
        }],
        provider: "anthropic".into(),
        model_id: "claude-sonnet-4-6".into(),
        usage: Usage {
            input: 100,
            output: 50,
            cache_read: 10,
            cache_write: 5,
            total: 165,
            ..Default::default()
        },
        cost: Cost {
            input: 0.01,
            output: 0.02,
            cache_read: 0.001,
            cache_write: 0.0005,
            total: 0.0315,
            ..Default::default()
        },
        stop_reason: StopReason::Stop,
        error_message: Some("optional error".into()),
        timestamp: 1710000001,
    };
    assert_eq!(msg.provider, "anthropic");
    assert_eq!(msg.model_id, "claude-sonnet-4-6");
    assert_eq!(msg.usage.input, 100);
    assert_eq!(msg.usage.output, 50);
    assert!((msg.cost.total - 0.0315).abs() < f64::EPSILON);
    assert_eq!(msg.stop_reason, StopReason::Stop);
    assert_eq!(msg.error_message.as_deref(), Some("optional error"));
    assert_eq!(msg.timestamp, 1710000001);
}

// T010
#[test]
fn tool_result_message_construction_and_access() {
    let msg = ToolResultMessage {
        tool_call_id: "tc_123".into(),
        content: vec![ContentBlock::Text {
            text: "tool output".into(),
        }],
        is_error: false,
        timestamp: 1710000002,
        details: serde_json::Value::Null,
    };
    assert_eq!(msg.tool_call_id, "tc_123");
    assert_eq!(msg.content.len(), 1);
    assert!(!msg.is_error);
    assert_eq!(msg.timestamp, 1710000002);
}

// T011
#[test]
fn message_conversation_sequence() {
    let messages: Vec<AgentMessage> = vec![
        AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "user input".into(),
            }],
            timestamp: 1,
        })),
        AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
            content: vec![ContentBlock::ToolCall {
                id: "tc_1".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "/tmp/file.txt"}),
                partial_json: None,
            }],
            provider: "anthropic".into(),
            model_id: "claude".into(),
            usage: Usage::default(),
            cost: Cost::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            timestamp: 2,
        })),
        AgentMessage::Llm(LlmMessage::ToolResult(ToolResultMessage {
            tool_call_id: "tc_1".into(),
            content: vec![ContentBlock::Text {
                text: "file contents".into(),
            }],
            is_error: false,
            timestamp: 3,
            details: serde_json::Value::Null,
        })),
        AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
            content: vec![ContentBlock::Text {
                text: "final answer".into(),
            }],
            provider: "anthropic".into(),
            model_id: "claude".into(),
            usage: Usage::default(),
            cost: Cost::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 4,
        })),
    ];
    assert_eq!(messages.len(), 4);
    assert!(matches!(&messages[0], AgentMessage::Llm(LlmMessage::User(_))));
    assert!(matches!(
        &messages[1],
        AgentMessage::Llm(LlmMessage::Assistant(_))
    ));
    assert!(matches!(
        &messages[2],
        AgentMessage::Llm(LlmMessage::ToolResult(_))
    ));
    assert!(matches!(
        &messages[3],
        AgentMessage::Llm(LlmMessage::Assistant(_))
    ));
}

// T012
#[test]
fn llm_message_serde_roundtrip() {
    let user = LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: "hello".into(),
        }],
        timestamp: 100,
    });
    let assistant = LlmMessage::Assistant(AssistantMessage {
        content: vec![ContentBlock::Text {
            text: "hi".into(),
        }],
        provider: "openai".into(),
        model_id: "gpt-4".into(),
        usage: Usage {
            input: 10,
            output: 20,
            cache_read: 0,
            cache_write: 0,
            total: 30,
            ..Default::default()
        },
        cost: Cost {
            input: 0.001,
            output: 0.002,
            cache_read: 0.0,
            cache_write: 0.0,
            total: 0.003,
            ..Default::default()
        },
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 101,
    });
    let tool_result = LlmMessage::ToolResult(ToolResultMessage {
        tool_call_id: "tc_1".into(),
        content: vec![ContentBlock::Text {
            text: "result".into(),
        }],
        is_error: true,
        timestamp: 102,
        details: serde_json::Value::Null,
    });

    for msg in [&user, &assistant, &tool_result] {
        let json = serde_json::to_string(msg).unwrap();
        let parsed: LlmMessage = serde_json::from_str(&json).unwrap();
        let re_json = serde_json::to_string(&parsed).unwrap();
        assert_eq!(json, re_json, "round-trip failed for {msg:?}");
    }
}

// T013 — verify UserMessage, AssistantMessage, ToolResultMessage derive Serialize/Deserialize
// (Covered implicitly by T012 round-trip test)

// T014 — verify LlmMessage has correct serde tagging
#[test]
fn llm_message_serde_tag() {
    let msg = LlmMessage::User(UserMessage {
        content: vec![],
        timestamp: 0,
    });
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["role"], "user");
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 4 — User Story 2: Rich Content Blocks
// ═══════════════════════════════════════════════════════════════════════════

// T016
#[test]
fn content_block_text_construction() {
    let block = ContentBlock::Text {
        text: "hello".into(),
    };
    assert!(matches!(block, ContentBlock::Text { text } if text == "hello"));
}

// T017
#[test]
fn content_block_thinking_with_signature() {
    let block = ContentBlock::Thinking {
        thinking: "reasoning step".into(),
        signature: Some("sig123".into()),
    };
    assert!(
        matches!(&block, ContentBlock::Thinking { thinking, signature }
            if thinking == "reasoning step" && signature.as_deref() == Some("sig123"))
    );

    let no_sig = ContentBlock::Thinking {
        thinking: "no sig".into(),
        signature: None,
    };
    assert!(
        matches!(&no_sig, ContentBlock::Thinking { signature, .. } if signature.is_none())
    );
}

// T018
#[test]
fn content_block_tool_call_with_empty_args() {
    let block = ContentBlock::ToolCall {
        id: "tc_1".into(),
        name: "read_file".into(),
        arguments: serde_json::json!({}),
        partial_json: None,
    };
    if let ContentBlock::ToolCall {
        id,
        name,
        arguments,
        partial_json,
    } = &block
    {
        assert_eq!(id, "tc_1");
        assert_eq!(name, "read_file");
        assert_eq!(arguments, &serde_json::json!({}));
        assert!(partial_json.is_none());
    } else {
        panic!("expected ToolCall variant");
    }
}

// T019
#[test]
fn content_block_image_all_sources() {
    // Base64
    let base64 = ContentBlock::Image {
        source: ImageSource::Base64 {
            media_type: "image/png".into(),
            data: "iVBORw0KGgo=".into(),
        },
    };
    assert!(
        matches!(&base64, ContentBlock::Image { source: ImageSource::Base64 { media_type, data } }
            if media_type == "image/png" && !data.is_empty())
    );

    // Url
    let url = ContentBlock::Image {
        source: ImageSource::Url {
            url: "https://example.com/img.png".into(),
            media_type: "image/png".into(),
        },
    };
    assert!(
        matches!(&url, ContentBlock::Image { source: ImageSource::Url { url, media_type } }
            if url.contains("example") && media_type == "image/png")
    );

    // File
    let file = ContentBlock::Image {
        source: ImageSource::File {
            path: std::path::PathBuf::from("/tmp/img.jpg"),
            media_type: "image/jpeg".into(),
        },
    };
    assert!(
        matches!(&file, ContentBlock::Image { source: ImageSource::File { path, media_type } }
            if path.to_str() == Some("/tmp/img.jpg") && media_type == "image/jpeg")
    );
}

// T020
#[test]
fn content_block_serde_roundtrip_all_variants() {
    let blocks = vec![
        ContentBlock::Text {
            text: "hello".into(),
        },
        ContentBlock::Thinking {
            thinking: "reason".into(),
            signature: Some("sig".into()),
        },
        ContentBlock::ToolCall {
            id: "tc_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"cmd": "ls"}),
            partial_json: Some("{\"cmd\":".into()),
        },
        ContentBlock::Image {
            source: ImageSource::Base64 {
                media_type: "image/png".into(),
                data: "data".into(),
            },
        },
        ContentBlock::Image {
            source: ImageSource::Url {
                url: "https://example.com/img.png".into(),
                media_type: "image/png".into(),
            },
        },
        ContentBlock::Image {
            source: ImageSource::File {
                path: std::path::PathBuf::from("/tmp/img.jpg"),
                media_type: "image/jpeg".into(),
            },
        },
    ];

    for block in &blocks {
        let json = serde_json::to_string(block).unwrap();
        let parsed: ContentBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(block, &parsed, "round-trip failed for {block:?}");
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 5 — User Story 3: Token Usage and Cost
// ═══════════════════════════════════════════════════════════════════════════

// T024
#[test]
fn usage_individual_counters() {
    let usage = Usage {
        input: 100,
        output: 200,
        cache_read: 50,
        cache_write: 25,
        total: 375,
        ..Default::default()
    };
    assert_eq!(usage.input, 100);
    assert_eq!(usage.output, 200);
    assert_eq!(usage.cache_read, 50);
    assert_eq!(usage.cache_write, 25);
    assert_eq!(usage.total, 375);
}

// T025
#[test]
fn usage_aggregation_two_records() {
    let a = Usage {
        input: 10,
        output: 20,
        cache_read: 5,
        cache_write: 3,
        total: 38,
        ..Default::default()
    };
    let b = Usage {
        input: 1,
        output: 2,
        cache_read: 3,
        cache_write: 4,
        total: 10,
        ..Default::default()
    };
    let c = a + b;
    assert_eq!(c.input, 11);
    assert_eq!(c.output, 22);
    assert_eq!(c.cache_read, 8);
    assert_eq!(c.cache_write, 7);
    assert_eq!(c.total, 48);
}

// T026
#[test]
fn usage_add_assign() {
    let mut a = Usage::default();
    let b = Usage {
        input: 5,
        output: 10,
        cache_read: 1,
        cache_write: 2,
        total: 18,
        ..Default::default()
    };
    a += b.clone();
    assert_eq!(a, b);
}

// T027
#[test]
fn usage_zero_counters_valid() {
    let usage = Usage::default();
    assert_eq!(usage.input, 0);
    assert_eq!(usage.output, 0);
    assert_eq!(usage.cache_read, 0);
    assert_eq!(usage.cache_write, 0);
    assert_eq!(usage.total, 0);
}

// T028
#[test]
fn cost_per_category_and_total() {
    let a = Cost {
        input: 0.01,
        output: 0.02,
        cache_read: 0.005,
        cache_write: 0.003,
        total: 0.038,
        ..Default::default()
    };
    let b = Cost {
        input: 0.01,
        output: 0.02,
        cache_read: 0.005,
        cache_write: 0.003,
        total: 0.038,
        ..Default::default()
    };
    let c = a + b;
    assert!((c.input - 0.02).abs() < f64::EPSILON);
    assert!((c.output - 0.04).abs() < f64::EPSILON);
    assert!((c.cache_read - 0.01).abs() < f64::EPSILON);
    assert!((c.cache_write - 0.006).abs() < f64::EPSILON);
    assert!((c.total - 0.076).abs() < f64::EPSILON);
}

// T029
#[test]
fn cost_add_assign() {
    let mut a = Cost::default();
    let b = Cost {
        input: 0.1,
        output: 0.2,
        cache_read: 0.0,
        cache_write: 0.0,
        total: 0.3,
        ..Default::default()
    };
    a += b;
    assert!((a.input - 0.1).abs() < f64::EPSILON);
    assert!((a.output - 0.2).abs() < f64::EPSILON);
    assert!((a.total - 0.3).abs() < f64::EPSILON);
}

// T030
#[test]
fn usage_cost_serde_roundtrip() {
    let usage = Usage {
        input: 100,
        output: 200,
        cache_read: 50,
        cache_write: 25,
        total: 375,
        ..Default::default()
    };
    let json = serde_json::to_string(&usage).unwrap();
    let parsed: Usage = serde_json::from_str(&json).unwrap();
    assert_eq!(usage, parsed);

    let cost = Cost {
        input: 0.01,
        output: 0.02,
        cache_read: 0.005,
        cache_write: 0.003,
        total: 0.038,
        ..Default::default()
    };
    let json = serde_json::to_string(&cost).unwrap();
    let parsed: Cost = serde_json::from_str(&json).unwrap();
    assert!((cost.input - parsed.input).abs() < f64::EPSILON);
    assert!((cost.total - parsed.total).abs() < f64::EPSILON);
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 7 — User Story 5: Custom Messages Extension
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug)]
struct TestCustom {
    value: String,
}

impl CustomMessage for TestCustom {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn type_name(&self) -> Option<&str> {
        Some("test_custom")
    }
}

#[derive(Debug)]
struct WrongType;
impl CustomMessage for WrongType {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// T043
#[test]
fn custom_message_wrap_and_store() {
    let custom = TestCustom {
        value: "data".into(),
    };
    let messages: Vec<AgentMessage> = vec![
        AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "hi".into(),
            }],
            timestamp: 0,
        })),
        AgentMessage::Custom(Box::new(custom)),
    ];
    assert_eq!(messages.len(), 2);
    assert!(matches!(&messages[1], AgentMessage::Custom(_)));
}

// T044
#[test]
fn custom_message_downcast_success() {
    let custom = TestCustom {
        value: "hello".into(),
    };
    let msg = AgentMessage::Custom(Box::new(custom));
    let result = msg.downcast_ref::<TestCustom>();
    assert!(result.is_ok());
    assert_eq!(result.unwrap().value, "hello");
}

// T045
#[test]
fn custom_message_downcast_wrong_type() {
    let custom = TestCustom {
        value: "hello".into(),
    };
    let msg = AgentMessage::Custom(Box::new(custom));
    let result = msg.downcast_ref::<WrongType>();
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("Downcast failed"));
}

// T046
#[test]
fn custom_message_downcast_on_llm_variant() {
    let msg = AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![],
        timestamp: 0,
    }));
    let result = msg.downcast_ref::<TestCustom>();
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.actual.contains("LlmMessage"));
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 8 — Remaining Types: StopReason, ModelSpec, AgentResult, AgentContext
// ═══════════════════════════════════════════════════════════════════════════

// T050
#[test]
fn stop_reason_all_variants() {
    let variants = [
        StopReason::Stop,
        StopReason::Length,
        StopReason::ToolUse,
        StopReason::Aborted,
        StopReason::Error,
    ];
    for reason in &variants {
        let json = serde_json::to_string(reason).unwrap();
        let parsed: StopReason = serde_json::from_str(&json).unwrap();
        assert_eq!(reason, &parsed);
    }
    // pattern matching
    assert!(matches!(variants[0], StopReason::Stop));
    assert!(matches!(variants[1], StopReason::Length));
    assert!(matches!(variants[2], StopReason::ToolUse));
    assert!(matches!(variants[3], StopReason::Aborted));
    assert!(matches!(variants[4], StopReason::Error));
}

// T051
#[test]
fn thinking_level_all_variants() {
    let levels = [
        ThinkingLevel::Off,
        ThinkingLevel::Minimal,
        ThinkingLevel::Low,
        ThinkingLevel::Medium,
        ThinkingLevel::High,
        ThinkingLevel::ExtraHigh,
    ];
    assert_eq!(ThinkingLevel::default(), ThinkingLevel::Off);
    for level in &levels {
        let json = serde_json::to_string(level).unwrap();
        let parsed: ThinkingLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(level, &parsed);
    }
}

// T052
#[test]
fn model_spec_construction_and_builder() {
    let mut map = HashMap::new();
    map.insert(ThinkingLevel::High, 10_000u64);
    let budgets = ThinkingBudgets::new(map);
    let caps = ModelCapabilities::none()
        .with_thinking(true)
        .with_vision(true);

    let spec = ModelSpec::new("anthropic", "claude-sonnet-4-6")
        .with_thinking_level(ThinkingLevel::High)
        .with_thinking_budgets(budgets)
        .with_provider_config(serde_json::json!({"temperature": 0.7}))
        .with_capabilities(caps.clone());

    assert_eq!(spec.provider, "anthropic");
    assert_eq!(spec.model_id, "claude-sonnet-4-6");
    assert_eq!(spec.thinking_level, ThinkingLevel::High);
    assert_eq!(
        spec.thinking_budgets.as_ref().unwrap().get(&ThinkingLevel::High),
        Some(10_000)
    );
    assert!(spec.provider_config.is_some());
    assert_eq!(spec.capabilities, Some(caps));
}

// T053
#[test]
fn model_spec_serde_roundtrip() {
    let mut map = HashMap::new();
    map.insert(ThinkingLevel::Medium, 5_000u64);
    let budgets = ThinkingBudgets::new(map);
    let caps = ModelCapabilities::none()
        .with_thinking(true)
        .with_streaming(true)
        .with_max_context_window(200_000);

    let spec = ModelSpec::new("anthropic", "claude-sonnet-4-6")
        .with_thinking_level(ThinkingLevel::Medium)
        .with_thinking_budgets(budgets)
        .with_provider_config(serde_json::json!({"temperature": 0.5}))
        .with_capabilities(caps);

    let json = serde_json::to_string(&spec).unwrap();
    let parsed: ModelSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(spec, parsed);
}

// T054
#[test]
fn agent_result_construction() {
    let result = AgentResult {
        messages: vec![AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
            content: vec![ContentBlock::Text {
                text: "answer".into(),
            }],
            provider: "openai".into(),
            model_id: "gpt-4".into(),
            usage: Usage {
                input: 50,
                output: 100,
                total: 150,
                ..Default::default()
            },
            cost: Cost {
                input: 0.005,
                output: 0.01,
                total: 0.015,
                ..Default::default()
            },
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 1,
        }))],
        stop_reason: StopReason::Stop,
        usage: Usage {
            input: 50,
            output: 100,
            total: 150,
            ..Default::default()
        },
        cost: Cost {
            input: 0.005,
            output: 0.01,
            total: 0.015,
            ..Default::default()
        },
        error: None,
    };
    assert_eq!(result.messages.len(), 1);
    assert_eq!(result.stop_reason, StopReason::Stop);
    assert_eq!(result.usage.total, 150);
    assert!(result.error.is_none());
}

// T055
#[test]
fn agent_context_construction() {
    let ctx = AgentContext {
        system_prompt: "You are a helpful assistant.".into(),
        messages: vec![
            AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
                timestamp: 0,
            })),
            AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
                content: vec![ContentBlock::Text {
                    text: "hi".into(),
                }],
                provider: "test".into(),
                model_id: "test-model".into(),
                usage: Usage::default(),
                cost: Cost::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 1,
            })),
        ],
        tools: vec![],
    };
    assert_eq!(ctx.system_prompt, "You are a helpful assistant.");
    assert_eq!(ctx.messages.len(), 2);
    assert!(ctx.tools.is_empty());
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 9 — Compile-time assertions (T059)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn all_public_types_are_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<ContentBlock>();
    assert_send_sync::<ImageSource>();
    assert_send_sync::<UserMessage>();
    assert_send_sync::<AssistantMessage>();
    assert_send_sync::<ToolResultMessage>();
    assert_send_sync::<LlmMessage>();
    assert_send_sync::<AgentMessage>();
    assert_send_sync::<Usage>();
    assert_send_sync::<Cost>();
    assert_send_sync::<StopReason>();
    assert_send_sync::<ThinkingLevel>();
    assert_send_sync::<ThinkingBudgets>();
    assert_send_sync::<ModelCapabilities>();
    assert_send_sync::<ModelSpec>();
    assert_send_sync::<AgentResult>();
    assert_send_sync::<AgentContext>();
    assert_send_sync::<DowncastError>();
}
