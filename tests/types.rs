use std::collections::HashMap;

use swink_agent::{
    AgentContext, AgentMessage, AssistantMessage, ContentBlock, Cost, CustomMessage, ImageSource,
    LlmMessage, ModelSpec, StopReason, ThinkingBudgets, ThinkingLevel, ToolResultMessage, Usage,
    UserMessage,
};

// ── 1.2: ContentBlock variants construct and pattern-match ──

#[test]
fn content_block_text() {
    let block = ContentBlock::Text {
        text: "hello".into(),
    };
    assert!(matches!(block, ContentBlock::Text { text } if text == "hello"));
}

#[test]
fn content_block_thinking() {
    let block = ContentBlock::Thinking {
        thinking: "reason".into(),
        signature: Some("sig".into()),
    };
    assert!(
        matches!(&block, ContentBlock::Thinking { thinking, signature }
            if thinking == "reason" && signature.as_deref() == Some("sig"))
    );
}

#[test]
fn content_block_tool_call() {
    let block = ContentBlock::ToolCall {
        id: "tc_1".into(),
        name: "read".into(),
        arguments: serde_json::json!({"path": "/tmp"}),
        partial_json: None,
    };
    assert!(
        matches!(&block, ContentBlock::ToolCall { id, name, .. } if id == "tc_1" && name == "read")
    );
}

#[test]
fn content_block_image() {
    let block = ContentBlock::Image {
        source: ImageSource::Url {
            url: "https://example.com/img.png".into(),
        },
    };
    assert!(
        matches!(&block, ContentBlock::Image { source: ImageSource::Url { url } } if url.contains("example"))
    );
}

// ── 1.3: LlmMessage wraps/unwraps each message type ──

#[test]
fn llm_message_user() {
    let msg = LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text { text: "hi".into() }],
        timestamp: 1,
    });
    assert!(matches!(msg, LlmMessage::User(_)));
}

#[test]
fn llm_message_assistant() {
    let msg = LlmMessage::Assistant(AssistantMessage {
        content: vec![],
        provider: "anthropic".into(),
        model_id: "claude".into(),
        usage: Usage::default(),
        cost: Cost::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 2,
    });
    assert!(matches!(msg, LlmMessage::Assistant(_)));
}

#[test]
fn llm_message_tool_result() {
    let msg = LlmMessage::ToolResult(ToolResultMessage {
        tool_call_id: "tc_1".into(),
        content: vec![ContentBlock::Text { text: "ok".into() }],
        is_error: false,
        timestamp: 3,
        details: serde_json::Value::Null,
    });
    assert!(matches!(msg, LlmMessage::ToolResult(_)));
}

// ── 1.4: AgentMessage::Custom holds a boxed trait object and downcasts ──

#[derive(Debug)]
struct TestCustomMessage {
    value: String,
}

impl CustomMessage for TestCustomMessage {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[test]
fn agent_message_custom_downcast() {
    let custom = TestCustomMessage {
        value: "hello".into(),
    };
    let msg = AgentMessage::Custom(Box::new(custom));

    if let AgentMessage::Custom(ref boxed) = msg {
        let downcasted = boxed.as_any().downcast_ref::<TestCustomMessage>();
        assert!(downcasted.is_some());
        assert_eq!(downcasted.unwrap().value, "hello");
    } else {
        panic!("expected Custom variant");
    }
}

// ── 1.5: Usage and Cost aggregate correctly ──

#[test]
fn usage_add() {
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

#[test]
fn usage_merge() {
    let mut a = Usage {
        input: 1,
        output: 1,
        cache_read: 1,
        cache_write: 1,
        total: 4,
        ..Default::default()
    };
    let b = Usage {
        input: 2,
        output: 2,
        cache_read: 2,
        cache_write: 2,
        total: 8,
        ..Default::default()
    };
    a.merge(&b);
    assert_eq!(a.input, 3);
    assert_eq!(a.total, 12);
}

#[test]
fn cost_add() {
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
    assert!((c.total - 0.076).abs() < f64::EPSILON);
}

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
    assert!((a.total - 0.3).abs() < f64::EPSILON);
}

// ── 1.6: StopReason and ThinkingLevel round-trip through serde ──

#[test]
fn stop_reason_serde_roundtrip() {
    for reason in [
        StopReason::Stop,
        StopReason::Length,
        StopReason::ToolUse,
        StopReason::Aborted,
        StopReason::Error,
    ] {
        let json = serde_json::to_string(&reason).unwrap();
        let parsed: StopReason = serde_json::from_str(&json).unwrap();
        assert_eq!(reason, parsed);
    }
}

#[test]
fn thinking_level_serde_roundtrip() {
    for level in [
        ThinkingLevel::Off,
        ThinkingLevel::Minimal,
        ThinkingLevel::Low,
        ThinkingLevel::Medium,
        ThinkingLevel::High,
        ThinkingLevel::ExtraHigh,
    ] {
        let json = serde_json::to_string(&level).unwrap();
        let parsed: ThinkingLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(level, parsed);
    }
}

// ── 1.7: ModelSpec constructs with defaults ──

#[test]
fn model_spec_defaults() {
    let spec = ModelSpec::new("anthropic", "claude-sonnet-4-6");
    assert_eq!(spec.provider, "anthropic");
    assert_eq!(spec.model_id, "claude-sonnet-4-6");
    assert_eq!(spec.thinking_level, ThinkingLevel::Off);
    assert!(spec.thinking_budgets.is_none());
}

// ── 1.8: ModelSpec builder methods ──

#[test]
fn model_spec_with_thinking_level() {
    let spec =
        ModelSpec::new("anthropic", "claude-sonnet-4-6").with_thinking_level(ThinkingLevel::High);
    assert_eq!(spec.thinking_level, ThinkingLevel::High);
}

#[test]
fn model_spec_with_thinking_budgets() {
    let mut map = HashMap::new();
    map.insert(ThinkingLevel::High, 10_000);
    let budgets = ThinkingBudgets::new(map);

    let spec = ModelSpec::new("anthropic", "claude-sonnet-4-6").with_thinking_budgets(budgets);
    assert!(spec.thinking_budgets.is_some());
    assert_eq!(
        spec.thinking_budgets.unwrap().get(&ThinkingLevel::High),
        Some(10_000)
    );
}

#[test]
fn model_spec_builder_chain() {
    let mut map = HashMap::new();
    map.insert(ThinkingLevel::Medium, 5_000);
    let budgets = ThinkingBudgets::new(map);

    let spec = ModelSpec::new("anthropic", "claude-sonnet-4-6")
        .with_thinking_level(ThinkingLevel::Medium)
        .with_thinking_budgets(budgets);

    assert_eq!(spec.provider, "anthropic");
    assert_eq!(spec.model_id, "claude-sonnet-4-6");
    assert_eq!(spec.thinking_level, ThinkingLevel::Medium);
    assert!(spec.thinking_budgets.is_some());
    assert_eq!(
        spec.thinking_budgets.unwrap().get(&ThinkingLevel::Medium),
        Some(5_000)
    );
}

// ── 1.10: AgentContext compiles with Vec<AgentMessage> and Vec<Arc<dyn Any>> ──

#[test]
fn agent_context_compiles() {
    let ctx = AgentContext {
        system_prompt: "You are helpful.".into(),
        messages: vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "hello".into(),
            }],
            timestamp: 0,
        }))],
        tools: vec![],
    };
    assert_eq!(ctx.messages.len(), 1);
    assert!(ctx.tools.is_empty());
}
