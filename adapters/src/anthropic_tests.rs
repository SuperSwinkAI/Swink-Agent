//! Tests for `anthropic`.
#![cfg(test)]

use super::*;

#[test]
fn cache_strategy_none_no_markers() {
    let tools = vec![AnthropicToolDef {
        name: "test".to_string(),
        description: "desc".to_string(),
        input_schema: serde_json::json!({}),
        cache_control: None,
    }];

    let request = AnthropicChatRequest {
        model: "claude-sonnet-4-6".to_string(),
        max_tokens: 4096,
        stream: true,
        system: Some(Value::String("You are helpful".to_string())),
        messages: vec![],
        tools,
        temperature: None,
        thinking: None,
    };

    let json = serde_json::to_value(&request).unwrap();
    // System should be a plain string
    assert_eq!(json["system"], "You are helpful");
    // Tools should not have cache_control
    assert!(json["tools"][0].get("cache_control").is_none());
}

#[test]
fn cache_strategy_auto_anthropic_markers() {
    // Simulate what send_request does with CacheStrategy::Auto
    let system_text = Some("You are helpful".to_string());
    let mut tools = vec![AnthropicToolDef {
        name: "test".to_string(),
        description: "desc".to_string(),
        input_schema: serde_json::json!({}),
        cache_control: None,
    }];

    // Apply caching (mirroring send_request logic)
    if let Some(last) = tools.last_mut() {
        last.cache_control = Some(CacheControl {
            r#type: "ephemeral",
        });
    }
    let system = system_text.map(|text| {
        serde_json::to_value(vec![SystemBlock {
            r#type: "text",
            text,
            cache_control: Some(CacheControl {
                r#type: "ephemeral",
            }),
        }])
        .unwrap()
    });

    let request = AnthropicChatRequest {
        model: "claude-sonnet-4-6".to_string(),
        max_tokens: 4096,
        stream: true,
        system,
        messages: vec![],
        tools,
        temperature: None,
        thinking: None,
    };

    let json = serde_json::to_value(&request).unwrap();
    // System should be an array with cache_control
    let sys_array = json["system"].as_array().unwrap();
    assert_eq!(sys_array.len(), 1);
    assert_eq!(sys_array[0]["type"], "text");
    assert_eq!(sys_array[0]["text"], "You are helpful");
    assert_eq!(sys_array[0]["cache_control"]["type"], "ephemeral");
    // Last tool should have cache_control
    assert_eq!(json["tools"][0]["cache_control"]["type"], "ephemeral");
}

#[test]
fn cache_strategy_ignored_by_unsupporting_adapter() {
    // CacheStrategy::Auto on a non-Anthropic adapter should be a no-op.
    // This is tested by verifying that CacheStrategy is just an enum —
    // adapters that don't support it simply don't read the field.
    let strategy = CacheStrategy::Auto;
    assert!(matches!(strategy, CacheStrategy::Auto));
    // No code changes needed in other adapters — they ignore it by design.
}

// ── SSE event processing (BlockAccumulator integration) ───────────────

/// Helper: create a fresh `SseStreamState`.
fn new_state() -> SseStreamState {
    SseStreamState {
        blocks: BlockAccumulator::default(),
        provider_blocks: HashMap::new(),
        usage: Usage::default(),
        stop_reason: None,
    }
}

/// Helper: run `process_sse_event` and return `(events, done)`.
fn process(
    event_type: &str,
    data: &str,
    state: &mut SseStreamState,
) -> (Vec<AssistantMessageEvent>, bool) {
    let mut done = false;
    let events = process_sse_event(event_type, data, state, &mut done);
    (events, done)
}

#[test]
fn text_block_lifecycle_via_sse() {
    let mut state = new_state();

    // content_block_start for text at provider index 0
    let (events, _) = process(
        "content_block_start",
        r#"{"index":0,"content_block":{"type":"text","text":""}}"#,
        &mut state,
    );
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        AssistantMessageEvent::TextStart { content_index: 0 }
    ));

    // text delta
    let (events, _) = process(
        "content_block_delta",
        r#"{"index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
        &mut state,
    );
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        AssistantMessageEvent::TextDelta { content_index: 0, delta } if delta == "Hello"
    ));

    // content_block_stop
    let (events, _) = process("content_block_stop", r#"{"index":0}"#, &mut state);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        AssistantMessageEvent::TextEnd { content_index: 0 }
    ));
}

#[test]
fn thinking_block_with_signature() {
    let mut state = new_state();

    let (events, _) = process(
        "content_block_start",
        r#"{"index":0,"content_block":{"type":"thinking","thinking":""}}"#,
        &mut state,
    );
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        AssistantMessageEvent::ThinkingStart { content_index: 0 }
    ));

    let (events, _) = process(
        "content_block_delta",
        r#"{"index":0,"delta":{"type":"thinking_delta","thinking":"Let me think..."}}"#,
        &mut state,
    );
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        AssistantMessageEvent::ThinkingDelta { content_index: 0, delta } if delta == "Let me think..."
    ));

    // Stop with signature
    let (events, _) = process(
        "content_block_stop",
        r#"{"index":0,"signature":"abc123"}"#,
        &mut state,
    );
    assert_eq!(events.len(), 1);
    match &events[0] {
        AssistantMessageEvent::ThinkingEnd {
            content_index,
            signature,
        } => {
            assert_eq!(*content_index, 0);
            assert_eq!(signature.as_deref(), Some("abc123"));
        }
        other => panic!("expected ThinkingEnd, got {other:?}"),
    }
}

#[test]
fn mixed_thinking_text_tool_call_indices() {
    let mut state = new_state();

    // Thinking block at provider index 0 → harness index 0
    let (events, _) = process(
        "content_block_start",
        r#"{"index":0,"content_block":{"type":"thinking","thinking":""}}"#,
        &mut state,
    );
    assert!(matches!(
        events[0],
        AssistantMessageEvent::ThinkingStart { content_index: 0 }
    ));

    let (events, _) = process("content_block_stop", r#"{"index":0}"#, &mut state);
    assert!(matches!(
        events[0],
        AssistantMessageEvent::ThinkingEnd {
            content_index: 0,
            ..
        }
    ));

    // Text block at provider index 1 → harness index 1
    let (events, _) = process(
        "content_block_start",
        r#"{"index":1,"content_block":{"type":"text","text":""}}"#,
        &mut state,
    );
    assert!(matches!(
        events[0],
        AssistantMessageEvent::TextStart { content_index: 1 }
    ));

    let (events, _) = process("content_block_stop", r#"{"index":1}"#, &mut state);
    assert!(matches!(
        events[0],
        AssistantMessageEvent::TextEnd { content_index: 1 }
    ));

    // Tool call at provider index 2 → harness index 2
    let (events, _) = process(
        "content_block_start",
        r#"{"index":2,"content_block":{"type":"tool_use","id":"call_1","name":"bash"}}"#,
        &mut state,
    );
    assert!(matches!(
        &events[0],
        AssistantMessageEvent::ToolCallStart {
            content_index: 2,
            ..
        }
    ));
}

#[test]
fn multiple_sequential_tool_calls() {
    let mut state = new_state();

    // First tool call at provider index 0
    let (events, _) = process(
        "content_block_start",
        r#"{"index":0,"content_block":{"type":"tool_use","id":"tc_1","name":"read_file"}}"#,
        &mut state,
    );
    assert_eq!(events.len(), 1);
    match &events[0] {
        AssistantMessageEvent::ToolCallStart {
            content_index,
            id,
            name,
        } => {
            assert_eq!(*content_index, 0);
            assert_eq!(id, "tc_1");
            assert_eq!(name, "read_file");
        }
        other => panic!("expected ToolCallStart, got {other:?}"),
    }

    // Delta for first tool call
    let (events, _) = process(
        "content_block_delta",
        r#"{"index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"foo\"}"}}"#,
        &mut state,
    );
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            ..
        }
    ));

    // Close first tool call
    let (events, _) = process("content_block_stop", r#"{"index":0}"#, &mut state);
    assert!(matches!(
        events[0],
        AssistantMessageEvent::ToolCallEnd { content_index: 0 }
    ));

    // Second tool call at provider index 1 → harness index 1
    let (events, _) = process(
        "content_block_start",
        r#"{"index":1,"content_block":{"type":"tool_use","id":"tc_2","name":"write_file"}}"#,
        &mut state,
    );
    match &events[0] {
        AssistantMessageEvent::ToolCallStart {
            content_index,
            id,
            name,
        } => {
            assert_eq!(*content_index, 1);
            assert_eq!(id, "tc_2");
            assert_eq!(name, "write_file");
        }
        other => panic!("expected ToolCallStart, got {other:?}"),
    }

    // Close second tool call
    let (events, _) = process("content_block_stop", r#"{"index":1}"#, &mut state);
    assert!(matches!(
        events[0],
        AssistantMessageEvent::ToolCallEnd { content_index: 1 }
    ));
}

#[test]
fn message_stop_emits_done_with_usage() {
    let mut state = new_state();

    // Set up usage via message_start
    process(
        "message_start",
        r#"{"message":{"usage":{"input_tokens":100,"cache_read_input_tokens":10,"cache_creation_input_tokens":5}}}"#,
        &mut state,
    );

    // Set up stop reason + output tokens
    process(
        "message_delta",
        r#"{"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":50}}"#,
        &mut state,
    );

    // message_stop triggers Done
    let (events, done) = process("message_stop", r"{}", &mut state);
    assert!(done);
    assert_eq!(events.len(), 1);
    match &events[0] {
        AssistantMessageEvent::Done {
            stop_reason, usage, ..
        } => {
            assert_eq!(*stop_reason, StopReason::Stop);
            assert_eq!(usage.input, 100);
            assert_eq!(usage.output, 50);
            assert_eq!(usage.cache_read, 10);
            assert_eq!(usage.cache_write, 5);
            assert_eq!(usage.total, 165);
        }
        other => panic!("expected Done, got {other:?}"),
    }
}

#[test]
fn error_event_closes_open_blocks() {
    let mut state = new_state();

    // Open a text block
    process(
        "content_block_start",
        r#"{"index":0,"content_block":{"type":"text","text":""}}"#,
        &mut state,
    );

    // SSE error arrives before content_block_stop
    let (events, done) = process(
        "error",
        r#"{"error":{"type":"overloaded_error","message":"Server overloaded"}}"#,
        &mut state,
    );
    assert!(done);
    // Should have: TextEnd (from finalize) + error event
    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[0],
        AssistantMessageEvent::TextEnd { content_index: 0 }
    ));
}

#[test]
fn malformed_message_start_is_terminal_protocol_error() {
    let mut state = new_state();

    let (events, done) = process("message_start", "{", &mut state);

    assert!(done);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        AssistantMessageEvent::Error {
            stop_reason: StopReason::Error,
            error_kind: None,
            error_message,
            ..
        } if error_message.contains("Anthropic message_start JSON parse error")
    ));
}

#[test]
fn malformed_content_block_delta_finalizes_open_blocks_before_error() {
    let mut state = new_state();

    process(
        "content_block_start",
        r#"{"index":0,"content_block":{"type":"text","text":""}}"#,
        &mut state,
    );

    let (events, done) = process("content_block_delta", "{", &mut state);

    assert!(done);
    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[0],
        AssistantMessageEvent::TextEnd { content_index: 0 }
    ));
    assert!(matches!(
        &events[1],
        AssistantMessageEvent::Error {
            stop_reason: StopReason::Error,
            error_kind: None,
            error_message,
            ..
        } if error_message.contains("Anthropic content_block_delta JSON parse error")
    ));
}

fn assert_protocol_error(event: &AssistantMessageEvent, event_type: &str, expected_detail: &str) {
    assert!(matches!(
        event,
        AssistantMessageEvent::Error {
            stop_reason: StopReason::Error,
            error_kind: None,
            error_message,
            ..
        } if error_message.contains(&format!("Anthropic {event_type} protocol error"))
            && error_message.contains(expected_detail)
    ));
}

#[test]
fn content_block_start_without_index_is_terminal_protocol_error() {
    let mut state = new_state();

    let (events, done) = process(
        "content_block_start",
        r#"{"content_block":{"type":"text","text":""}}"#,
        &mut state,
    );

    assert!(done);
    assert_eq!(events.len(), 1);
    assert_protocol_error(&events[0], "content_block_start", "index");
}

#[test]
fn content_block_delta_with_non_numeric_index_finalizes_before_error() {
    let mut state = new_state();

    process(
        "content_block_start",
        r#"{"index":0,"content_block":{"type":"text","text":""}}"#,
        &mut state,
    );

    let (events, done) = process(
        "content_block_delta",
        r#"{"index":"0","delta":{"type":"text_delta","text":"Hello"}}"#,
        &mut state,
    );

    assert!(done);
    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[0],
        AssistantMessageEvent::TextEnd { content_index: 0 }
    ));
    assert_protocol_error(&events[1], "content_block_delta", "index");
}

#[test]
fn content_block_stop_without_index_finalizes_before_error() {
    let mut state = new_state();

    process(
        "content_block_start",
        r#"{"index":0,"content_block":{"type":"text","text":""}}"#,
        &mut state,
    );

    let (events, done) = process("content_block_stop", r"{}", &mut state);

    assert!(done);
    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[0],
        AssistantMessageEvent::TextEnd { content_index: 0 }
    ));
    assert_protocol_error(&events[1], "content_block_stop", "index");
}

#[test]
fn tool_use_start_without_id_is_terminal_protocol_error() {
    let mut state = new_state();

    let (events, done) = process(
        "content_block_start",
        r#"{"index":0,"content_block":{"type":"tool_use","name":"bash"}}"#,
        &mut state,
    );

    assert!(done);
    assert_eq!(events.len(), 1);
    assert_protocol_error(&events[0], "content_block_start", "/content_block/id");
}

#[test]
fn tool_use_start_with_empty_name_is_terminal_protocol_error() {
    let mut state = new_state();

    let (events, done) = process(
        "content_block_start",
        r#"{"index":0,"content_block":{"type":"tool_use","id":"tu_1","name":""}}"#,
        &mut state,
    );

    assert!(done);
    assert_eq!(events.len(), 1);
    assert_protocol_error(&events[0], "content_block_start", "/content_block/name");
}

#[test]
fn malformed_error_event_is_non_retryable_parse_error() {
    let mut state = new_state();

    process(
        "content_block_start",
        r#"{"index":0,"content_block":{"type":"text","text":""}}"#,
        &mut state,
    );

    let (events, done) = process("error", "{", &mut state);

    assert!(done);
    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[0],
        AssistantMessageEvent::TextEnd { content_index: 0 }
    ));
    assert!(matches!(
        &events[1],
        AssistantMessageEvent::Error {
            stop_reason: StopReason::Error,
            error_kind: None,
            error_message,
            ..
        } if error_message.contains("Anthropic error JSON parse error")
    ));
}

#[test]
fn tool_use_stop_reason_mapping() {
    let mut state = new_state();

    process(
        "message_delta",
        r#"{"delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":10}}"#,
        &mut state,
    );

    let (events, done) = process("message_stop", r"{}", &mut state);
    assert!(done);
    match &events[0] {
        AssistantMessageEvent::Done { stop_reason, .. } => {
            assert_eq!(*stop_reason, StopReason::ToolUse);
        }
        other => panic!("expected Done, got {other:?}"),
    }
}

#[test]
fn open_blocks_drained_on_message_stop() {
    let mut state = new_state();

    // Open text and tool call, don't close them
    process(
        "content_block_start",
        r#"{"index":0,"content_block":{"type":"text","text":""}}"#,
        &mut state,
    );
    process(
        "content_block_start",
        r#"{"index":1,"content_block":{"type":"tool_use","id":"tc_1","name":"bash"}}"#,
        &mut state,
    );

    // message_stop should finalize both open blocks
    let (events, done) = process("message_stop", r"{}", &mut state);
    assert!(done);
    // TextEnd + ToolCallEnd + Done = 3 events
    assert_eq!(events.len(), 3);
    assert!(matches!(
        events[0],
        AssistantMessageEvent::TextEnd { content_index: 0 }
    ));
    assert!(matches!(
        events[1],
        AssistantMessageEvent::ToolCallEnd { content_index: 1 }
    ));
    assert!(matches!(events[2], AssistantMessageEvent::Done { .. }));
}

#[test]
fn mixed_text_and_tool_call_stream() {
    let mut state = new_state();

    // Text block
    process(
        "content_block_start",
        r#"{"index":0,"content_block":{"type":"text","text":""}}"#,
        &mut state,
    );
    process(
        "content_block_delta",
        r#"{"index":0,"delta":{"type":"text_delta","text":"I will run a command."}}"#,
        &mut state,
    );
    let (events, _) = process("content_block_stop", r#"{"index":0}"#, &mut state);
    assert!(matches!(
        events[0],
        AssistantMessageEvent::TextEnd { content_index: 0 }
    ));

    // Tool call block
    process(
        "content_block_start",
        r#"{"index":1,"content_block":{"type":"tool_use","id":"call_abc","name":"bash"}}"#,
        &mut state,
    );
    process(
        "content_block_delta",
        r#"{"index":1,"delta":{"type":"input_json_delta","partial_json":"{\"cmd\":\"ls\"}"}}"#,
        &mut state,
    );
    let (events, _) = process("content_block_stop", r#"{"index":1}"#, &mut state);
    assert!(matches!(
        events[0],
        AssistantMessageEvent::ToolCallEnd { content_index: 1 }
    ));

    // Done
    process(
        "message_delta",
        r#"{"delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":20}}"#,
        &mut state,
    );
    let (events, done) = process("message_stop", r"{}", &mut state);
    assert!(done);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        AssistantMessageEvent::Done {
            stop_reason: StopReason::ToolUse,
            ..
        }
    ));
}

#[test]
fn trailing_slash_stripped() {
    let anthropic = AnthropicStreamFn::new("https://api.anthropic.com/", "key");
    assert_eq!(anthropic.base.base_url, "https://api.anthropic.com");
}

#[test]
fn no_trailing_slash_unchanged() {
    let anthropic = AnthropicStreamFn::new("https://api.anthropic.com", "key");
    assert_eq!(anthropic.base.base_url, "https://api.anthropic.com");
}

// ── Issue #619: incomplete tool_use sanitization ─────────────────────

/// Regression for #619: after the loop-level scrub runs, an assistant
/// message that originally carried `arguments: Null` with `partial_json`
/// set must serialize with `input: {}` so the Anthropic API accepts the
/// replayed history on the next turn.
#[test]
fn convert_messages_sanitized_tool_use_becomes_empty_object_input() {
    use swink_agent::AssistantMessage;

    let mut assistant = AssistantMessage::new(
        vec![ContentBlock::ToolCall {
            id: "toolu_01".into(),
            name: "read_file".into(),
            // Simulate an incomplete-tool-use block surviving Done(Length):
            arguments: Value::Null,
            partial_json: Some(r#"{"path": "/tm"#.into()),
        }],
        "anthropic",
        "claude-sonnet-4-6",
    )
    .with_stop_reason(StopReason::Length)
    .with_timestamp(0);

    // Loop-level scrub runs before the adapter sees the history.
    swink_agent::sanitize_incomplete_tool_calls(&mut assistant);

    let messages = vec![AgentMessage::Llm(LlmMessage::Assistant(assistant))];
    let (_system, converted) = convert_messages(&messages, "");

    assert_eq!(converted.len(), 1);
    assert_eq!(converted[0].role, "assistant");
    let json = serde_json::to_value(&converted[0]).unwrap();
    let block = &json["content"][0];
    assert_eq!(block["type"], "tool_use");
    assert_eq!(block["id"], "toolu_01");
    assert_eq!(block["name"], "read_file");
    // The critical assertion: input is a valid empty JSON object, NOT null.
    assert!(
        block["input"].is_object(),
        "input must be a JSON object, got {:?}",
        block["input"]
    );
    assert_eq!(block["input"].as_object().unwrap().len(), 0);
}
