use swink_agent::{
    AssistantMessageEvent, ContentBlock, Cost, StopReason, StreamOptions, StreamTransport, Usage,
    accumulate_message,
};

// ── 2.5: Event stream accumulates into correct AssistantMessage (text + tool call) ──

#[test]
fn accumulate_text_and_tool_call() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::TextStart { content_index: 0 },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "Hello".into(),
        },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: " world".into(),
        },
        AssistantMessageEvent::TextEnd { content_index: 0 },
        AssistantMessageEvent::ToolCallStart {
            content_index: 1,
            id: "tc_1".into(),
            name: "search".into(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 1,
            delta: r#"{"q":"#.into(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 1,
            delta: r#""rust"}"#.into(),
        },
        AssistantMessageEvent::ToolCallEnd { content_index: 1 },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::ToolUse,
            usage: Usage {
                input: 100,
                output: 50,
                cache_read: 0,
                cache_write: 0,
                total: 150,
                ..Default::default()
            },
            cost: Cost {
                input: 0.01,
                output: 0.02,
                cache_read: 0.0,
                cache_write: 0.0,
                total: 0.03,
                ..Default::default()
            },
        },
    ];

    let msg = accumulate_message(events, "anthropic", "claude-sonnet-4-6").unwrap();

    assert_eq!(msg.content.len(), 2);
    assert_eq!(msg.provider, "anthropic");
    assert_eq!(msg.model_id, "claude-sonnet-4-6");
    assert_eq!(msg.stop_reason, StopReason::ToolUse);
    assert_eq!(msg.usage.input, 100);
    assert_eq!(msg.usage.output, 50);
    assert_eq!(msg.usage.total, 150);
    assert!((msg.cost.total - 0.03).abs() < f64::EPSILON);
    assert!(msg.error_message.is_none());

    match &msg.content[0] {
        ContentBlock::Text { text } => assert_eq!(text, "Hello world"),
        other => panic!("expected Text, got {other:?}"),
    }

    match &msg.content[1] {
        ContentBlock::ToolCall {
            id,
            name,
            arguments,
            partial_json,
        } => {
            assert_eq!(id, "tc_1");
            assert_eq!(name, "search");
            assert_eq!(arguments, &serde_json::json!({"q": "rust"}));
            assert!(partial_json.is_none());
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
}

// ── 2.6: Interleaved text and tool call blocks accumulate correctly ──

#[test]
fn accumulate_interleaved_text_and_tool_calls() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ThinkingStart { content_index: 0 },
        AssistantMessageEvent::ThinkingDelta {
            content_index: 0,
            delta: "Let me think".into(),
        },
        AssistantMessageEvent::ThinkingDelta {
            content_index: 0,
            delta: " about this.".into(),
        },
        AssistantMessageEvent::ThinkingEnd {
            content_index: 0,
            signature: Some("sig123".into()),
        },
        AssistantMessageEvent::TextStart { content_index: 1 },
        AssistantMessageEvent::TextDelta {
            content_index: 1,
            delta: "I'll search for that.".into(),
        },
        AssistantMessageEvent::TextEnd { content_index: 1 },
        AssistantMessageEvent::ToolCallStart {
            content_index: 2,
            id: "tc_a".into(),
            name: "web_search".into(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 2,
            delta: r#"{"query": "rust async"}"#.into(),
        },
        AssistantMessageEvent::ToolCallEnd { content_index: 2 },
        AssistantMessageEvent::ToolCallStart {
            content_index: 3,
            id: "tc_b".into(),
            name: "read_file".into(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 3,
            delta: r#"{"path": "/tmp/foo.rs"}"#.into(),
        },
        AssistantMessageEvent::ToolCallEnd { content_index: 3 },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::ToolUse,
            usage: Usage {
                input: 200,
                output: 100,
                cache_read: 10,
                cache_write: 5,
                total: 315,
                ..Default::default()
            },
            cost: Cost::default(),
        },
    ];

    let msg = accumulate_message(events, "openai", "gpt-4").unwrap();

    assert_eq!(msg.content.len(), 4);
    assert_eq!(msg.stop_reason, StopReason::ToolUse);

    match &msg.content[0] {
        ContentBlock::Thinking {
            thinking,
            signature,
        } => {
            assert_eq!(thinking, "Let me think about this.");
            assert_eq!(signature.as_deref(), Some("sig123"));
        }
        other => panic!("expected Thinking, got {other:?}"),
    }

    match &msg.content[1] {
        ContentBlock::Text { text } => assert_eq!(text, "I'll search for that."),
        other => panic!("expected Text, got {other:?}"),
    }

    match &msg.content[2] {
        ContentBlock::ToolCall {
            id,
            name,
            arguments,
            ..
        } => {
            assert_eq!(id, "tc_a");
            assert_eq!(name, "web_search");
            assert_eq!(arguments, &serde_json::json!({"query": "rust async"}));
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }

    match &msg.content[3] {
        ContentBlock::ToolCall {
            id,
            name,
            arguments,
            ..
        } => {
            assert_eq!(id, "tc_b");
            assert_eq!(name, "read_file");
            assert_eq!(arguments, &serde_json::json!({"path": "/tmp/foo.rs"}));
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
}

// ── 2.12: StreamOptions defaults are sensible ──

#[test]
fn stream_options_defaults() {
    let opts = StreamOptions::default();
    assert!(opts.temperature.is_none());
    assert!(opts.max_tokens.is_none());
    assert!(opts.session_id.is_none());
    assert!(opts.api_key.is_none());
    assert_eq!(opts.transport, StreamTransport::Sse);
}

#[test]
fn stream_options_debug_redacts_api_key() {
    let opts = StreamOptions {
        api_key: Some("secret-key-123".to_string()),
        ..StreamOptions::default()
    };
    let debug = format!("{opts:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains("secret-key-123"));
}

// ── Additional: StreamTransport serde round-trip ──

#[test]
fn stream_transport_serde_roundtrip() {
    let transport = StreamTransport::Sse;
    let json = serde_json::to_string(&transport).unwrap();
    assert_eq!(json, r#""sse""#);
    let parsed: StreamTransport = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, transport);
}

// ── Error event accumulation ──

#[test]
fn accumulate_error_event() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::TextStart { content_index: 0 },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "partial".into(),
        },
        AssistantMessageEvent::TextEnd { content_index: 0 },
        AssistantMessageEvent::Error {
            stop_reason: StopReason::Error,
            error_message: "connection lost".into(),
            usage: Some(Usage {
                input: 50,
                output: 10,
                cache_read: 0,
                cache_write: 0,
                total: 60,
                ..Default::default()
            }),
            error_kind: None,
        },
    ];

    let msg = accumulate_message(events, "anthropic", "claude").unwrap();
    assert_eq!(msg.stop_reason, StopReason::Error);
    assert_eq!(msg.error_message.as_deref(), Some("connection lost"));
    assert_eq!(msg.usage.total, 60);
    assert_eq!(msg.content.len(), 1);
}

// ── Malformed events produce errors ──

#[test]
fn accumulate_no_start_event() {
    let events = vec![AssistantMessageEvent::TextStart { content_index: 0 }];
    let result = accumulate_message(events, "p", "m");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("before Start"));
}

#[test]
fn accumulate_no_terminal_event() {
    let events = vec![AssistantMessageEvent::Start];
    let result = accumulate_message(events, "p", "m");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("terminal event"));
}

#[test]
fn accumulate_wrong_content_index() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::TextStart { content_index: 1 },
    ];
    let result = accumulate_message(events, "p", "m");
    assert!(result.is_err());
}

#[test]
fn accumulate_delta_on_wrong_block_type() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::TextStart { content_index: 0 },
        AssistantMessageEvent::ThinkingDelta {
            content_index: 0,
            delta: "oops".into(),
        },
    ];
    let result = accumulate_message(events, "p", "m");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not Thinking"));
}

#[test]
fn accumulate_tool_call_empty_args() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            id: "tc_0".into(),
            name: "noop".into(),
        },
        AssistantMessageEvent::ToolCallEnd { content_index: 0 },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
            cost: Cost::default(),
        },
    ];

    let msg = accumulate_message(events, "p", "m").unwrap();
    match &msg.content[0] {
        ContentBlock::ToolCall { arguments, .. } => {
            assert!(arguments.is_object());
            assert!(arguments.as_object().unwrap().is_empty());
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
}

#[test]
fn accumulate_error_event_without_usage() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::Error {
            stop_reason: StopReason::Error,
            error_message: "fatal".into(),
            usage: None,
            error_kind: None,
        },
    ];

    let msg = accumulate_message(events, "p", "m").unwrap();
    assert_eq!(msg.stop_reason, StopReason::Error);
    assert_eq!(msg.error_message.as_deref(), Some("fatal"));
    assert_eq!(msg.usage, Usage::default());
}

// ── Additional coverage tests ──

#[test]
fn duplicate_start_event_errors() {
    let events = vec![AssistantMessageEvent::Start, AssistantMessageEvent::Start];
    let result = accumulate_message(events, "p", "m");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("duplicate Start"));
}

#[test]
fn tool_call_delta_invalid_index() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ToolCallDelta {
            content_index: 99,
            delta: "oops".into(),
        },
    ];
    let result = accumulate_message(events, "p", "m");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("invalid content_index"));
}

#[test]
fn thinking_only_response() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ThinkingStart { content_index: 0 },
        AssistantMessageEvent::ThinkingDelta {
            content_index: 0,
            delta: "Let me ".into(),
        },
        AssistantMessageEvent::ThinkingDelta {
            content_index: 0,
            delta: "ponder this.".into(),
        },
        AssistantMessageEvent::ThinkingEnd {
            content_index: 0,
            signature: None,
        },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            usage: Usage::default(),
            cost: Cost::default(),
        },
    ];

    let msg = accumulate_message(events, "anthropic", "claude").unwrap();
    assert_eq!(msg.content.len(), 1);
    assert_eq!(msg.stop_reason, StopReason::Stop);
    assert!(msg.error_message.is_none());

    match &msg.content[0] {
        ContentBlock::Thinking {
            thinking,
            signature,
        } => {
            assert_eq!(thinking, "Let me ponder this.");
            assert!(signature.is_none());
        }
        other => panic!("expected Thinking, got {other:?}"),
    }
}

#[test]
fn error_event_with_usage() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::Error {
            stop_reason: StopReason::Error,
            error_message: "overloaded".into(),
            usage: Some(Usage {
                input: 42,
                output: 7,
                cache_read: 0,
                cache_write: 0,
                total: 49,
                ..Default::default()
            }),
            error_kind: Some(swink_agent::StreamErrorKind::Throttled),
        },
    ];

    let msg = accumulate_message(events, "p", "m").unwrap();
    assert_eq!(msg.stop_reason, StopReason::Error);
    assert_eq!(msg.error_message.as_deref(), Some("overloaded"));
    assert_eq!(msg.usage.input, 42);
    assert_eq!(msg.usage.output, 7);
    assert_eq!(msg.usage.total, 49);
}

#[test]
fn text_delta_on_wrong_block_type() {
    // Create a Thinking block at index 0, then send a TextDelta targeting it.
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ThinkingStart { content_index: 0 },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "wrong target".into(),
        },
    ];
    let result = accumulate_message(events, "p", "m");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not Text"));
}

// ── T028: Empty stream edge case ──

#[test]
fn accumulate_empty_stream() {
    let events = vec![];
    let result = accumulate_message(events, "p", "m");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("no Start event"));
}

// ── T029: AssistantMessageDelta construction ──

#[test]
fn assistant_message_delta_variants() {
    use std::borrow::Cow;
    use swink_agent::AssistantMessageDelta;

    let text = AssistantMessageDelta::Text {
        content_index: 0,
        delta: Cow::Borrowed("hello"),
    };
    assert!(matches!(
        text,
        AssistantMessageDelta::Text {
            content_index: 0,
            ..
        }
    ));

    let thinking = AssistantMessageDelta::Thinking {
        content_index: 1,
        delta: Cow::Owned("pondering".to_string()),
    };
    assert!(matches!(
        thinking,
        AssistantMessageDelta::Thinking {
            content_index: 1,
            ..
        }
    ));

    let tool = AssistantMessageDelta::ToolCall {
        content_index: 2,
        delta: Cow::Borrowed(r#"{"key":"val"}"#),
    };
    assert!(matches!(
        tool,
        AssistantMessageDelta::ToolCall {
            content_index: 2,
            ..
        }
    ));
}
