//! Tests for `openai_compat`.
#![cfg(test)]

use super::*;

/// Helper: create an `OaiChunk` with one choice containing the given delta.
fn chunk_with_delta(delta: OaiDelta, finish_reason: Option<&str>) -> OaiChunk {
    OaiChunk {
        choices: vec![OaiChoice {
            delta,
            finish_reason: finish_reason.map(String::from),
            content_filter_results: None,
        }],
        usage: None,
    }
}

#[test]
fn reasoning_content_emits_thinking_events() {
    let mut state = OaiSseStreamState::default();
    let mut events = Vec::new();

    // First reasoning chunk → ThinkingStart + ThinkingDelta
    let chunk = chunk_with_delta(
        OaiDelta {
            reasoning_content: Some("Let me think".to_string()),
            ..Default::default()
        },
        None,
    );
    process_oai_chunk(&chunk, &mut state, &mut events, "test");

    assert_eq!(events.len(), 2);
    assert!(matches!(
        &events[0],
        AssistantMessageEvent::ThinkingStart { content_index: 0 }
    ));
    assert!(
        matches!(&events[1], AssistantMessageEvent::ThinkingDelta { content_index: 0, delta } if delta == "Let me think")
    );

    // Second reasoning chunk → only ThinkingDelta (no new Start)
    events.clear();
    let chunk = chunk_with_delta(
        OaiDelta {
            reasoning_content: Some(" more".to_string()),
            ..Default::default()
        },
        None,
    );
    process_oai_chunk(&chunk, &mut state, &mut events, "test");

    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], AssistantMessageEvent::ThinkingDelta { content_index: 0, delta } if delta == " more")
    );
}

#[test]
fn reasoning_to_content_transition_closes_thinking() {
    let mut state = OaiSseStreamState::default();
    let mut events = Vec::new();

    // Reasoning chunk
    let chunk = chunk_with_delta(
        OaiDelta {
            reasoning_content: Some("thinking...".to_string()),
            ..Default::default()
        },
        None,
    );
    process_oai_chunk(&chunk, &mut state, &mut events, "test");
    assert_eq!(events.len(), 2); // ThinkingStart + ThinkingDelta

    // Now regular content arrives → should close thinking, then open text
    events.clear();
    let chunk = chunk_with_delta(
        OaiDelta {
            content: Some("Hello".to_string()),
            ..Default::default()
        },
        None,
    );
    process_oai_chunk(&chunk, &mut state, &mut events, "test");

    // ThinkingEnd + TextStart + TextDelta
    assert_eq!(events.len(), 3);
    assert!(matches!(
        &events[0],
        AssistantMessageEvent::ThinkingEnd {
            content_index: 0,
            ..
        }
    ));
    assert!(matches!(
        &events[1],
        AssistantMessageEvent::TextStart { content_index: 1 }
    ));
    assert!(matches!(
        &events[2],
        AssistantMessageEvent::TextDelta { content_index: 1, delta } if delta == "Hello"
    ));
}

#[test]
fn reasoning_to_tool_call_closes_thinking() {
    let mut state = OaiSseStreamState::default();
    let mut events = Vec::new();

    // Reasoning chunk
    let chunk = chunk_with_delta(
        OaiDelta {
            reasoning_content: Some("planning...".to_string()),
            ..Default::default()
        },
        None,
    );
    process_oai_chunk(&chunk, &mut state, &mut events, "test");
    events.clear();

    // Tool call arrives
    let chunk = chunk_with_delta(
        OaiDelta {
            tool_calls: Some(vec![OaiToolCallDelta {
                index: 0,
                id: Some("call_1".to_string()),
                function: Some(OaiFunctionDelta {
                    name: Some("my_tool".to_string()),
                    arguments: Some(r#"{"a":1}"#.to_string()),
                }),
            }]),
            ..Default::default()
        },
        None,
    );
    process_oai_chunk(&chunk, &mut state, &mut events, "test");

    // First event should be ThinkingEnd
    assert!(matches!(
        &events[0],
        AssistantMessageEvent::ThinkingEnd {
            content_index: 0,
            ..
        }
    ));
    // Then ToolCallStart
    assert!(matches!(
        &events[1],
        AssistantMessageEvent::ToolCallStart {
            content_index: 1,
            ..
        }
    ));
}

#[test]
fn chunks_without_reasoning_work_normally() {
    let mut state = OaiSseStreamState::default();
    let mut events = Vec::new();

    // Regular text chunk
    let chunk = chunk_with_delta(
        OaiDelta {
            content: Some("Hello world".to_string()),
            ..Default::default()
        },
        None,
    );
    process_oai_chunk(&chunk, &mut state, &mut events, "test");

    assert_eq!(events.len(), 2); // TextStart + TextDelta
    assert!(matches!(
        &events[0],
        AssistantMessageEvent::TextStart { content_index: 0 }
    ));
    assert!(matches!(
        &events[1],
        AssistantMessageEvent::TextDelta { content_index: 0, delta } if delta == "Hello world"
    ));
}

#[test]
fn empty_reasoning_content_ignored() {
    let mut state = OaiSseStreamState::default();
    let mut events = Vec::new();

    let chunk = chunk_with_delta(
        OaiDelta {
            reasoning_content: Some(String::new()),
            ..Default::default()
        },
        None,
    );
    process_oai_chunk(&chunk, &mut state, &mut events, "test");

    assert!(events.is_empty());
}

#[test]
fn null_reasoning_content_ignored() {
    let mut state = OaiSseStreamState::default();
    let mut events = Vec::new();

    let chunk = chunk_with_delta(
        OaiDelta {
            reasoning_content: None,
            content: Some("text".to_string()),
            ..Default::default()
        },
        None,
    );
    process_oai_chunk(&chunk, &mut state, &mut events, "test");

    // Should just get text events, no thinking
    assert_eq!(events.len(), 2);
    assert!(matches!(
        &events[0],
        AssistantMessageEvent::TextStart { content_index: 0 }
    ));
}

#[test]
fn reasoning_content_deserialized_from_json() {
    let json = r#"{
            "choices": [{
                "delta": {
                    "reasoning_content": "step by step"
                },
                "finish_reason": null
            }]
        }"#;

    let chunk: OaiChunk = serde_json::from_str(json).unwrap();
    assert_eq!(chunk.choices.len(), 1);
    assert_eq!(
        chunk.choices[0].delta.reasoning_content.as_deref(),
        Some("step by step")
    );
}

// ── Issue #619: incomplete tool_use sanitization ─────────────────────

/// Regression for #619: after the loop-level scrub runs, an assistant
/// message that originally carried `arguments: Null` with `partial_json`
/// set must serialize with `function.arguments: "{}"` (a stringified empty
/// object) rather than the literal string `"null"` that `Value::Null.to_string()`
/// would produce. Otherwise OpenAI-compatible providers reject the request
/// when they try to parse the arguments.
#[test]
fn assistant_message_sanitized_tool_call_serializes_empty_object_string() {
    use swink_agent::AssistantMessage;

    let mut assistant = AssistantMessage::new(
        vec![ContentBlock::ToolCall {
            id: "call_01".into(),
            name: "read_file".into(),
            arguments: Value::Null,
            partial_json: Some(r#"{"path": "/tm"#.into()),
        }],
        "openai",
        "gpt-4o-mini",
    )
    .with_stop_reason(StopReason::Length)
    .with_timestamp(0);

    swink_agent::sanitize_incomplete_tool_calls(&mut assistant);

    let oai_msg = OaiConverter::assistant_message(&assistant);
    assert_eq!(oai_msg.role, "assistant");
    let tool_calls = oai_msg.tool_calls.expect("tool_calls must be Some");
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].function.name, "read_file");
    // Critical: the stringified arguments must be a valid JSON object, NOT
    // the literal "null" that `Value::Null.to_string()` would produce.
    assert_eq!(tool_calls[0].function.arguments, "{}");
}

#[test]
fn terminal_parse_error_flushes_pending_tool_call_before_generic_error() {
    let mut state = OaiSseStreamState::default();
    state.tool_calls.insert(
        0,
        OaiToolCallEntry {
            id: "call_1".into(),
            name: Some("read_file".into()),
            arguments: r#"{"path":"foo.rs"}"#.into(),
            content_index: None,
        },
    );

    let events = state.emit_terminal_error(AssistantMessageEvent::error(
        "OpenAI JSON parse error: bad payload",
    ));
    let delta_index = events
        .iter()
        .position(|event| matches!(event, AssistantMessageEvent::ToolCallDelta { .. }))
        .expect("final tool delta");
    let end_index = events
        .iter()
        .position(|event| matches!(event, AssistantMessageEvent::ToolCallEnd { .. }))
        .expect("tool call end");
    let error_index = events
        .iter()
        .position(|event| matches!(event, AssistantMessageEvent::Error { .. }))
        .expect("terminal error");

    assert!(
        delta_index < end_index && end_index < error_index,
        "pending tool-call state must flush before the terminal error: {events:?}"
    );
    match &events[error_index] {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert!(
                error_kind.is_none(),
                "JSON parse errors must be non-retryable protocol errors"
            );
        }
        event => panic!("expected terminal error, got {event:?}"),
    }
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AssistantMessageEvent::Done { .. })),
        "terminal error path must not emit Done"
    );
}

#[test]
fn unexpected_eof_flushes_pending_tool_call_before_network_error() {
    let mut state = OaiSseStreamState::default();
    state.tool_calls.insert(
        0,
        OaiToolCallEntry {
            id: "call_1".into(),
            name: Some("read_file".into()),
            arguments: r#"{"path":"foo.rs"}"#.into(),
            content_index: None,
        },
    );

    let events = state.emit_terminal_error(AssistantMessageEvent::error_network(
        "OpenAI-compatible stream ended unexpectedly",
    ));
    let delta_index = events
        .iter()
        .position(|event| matches!(event, AssistantMessageEvent::ToolCallDelta { .. }))
        .expect("final tool delta");
    let end_index = events
        .iter()
        .position(|event| matches!(event, AssistantMessageEvent::ToolCallEnd { .. }))
        .expect("tool call end");
    let error_index = events
        .iter()
        .position(|event| matches!(event, AssistantMessageEvent::Error { .. }))
        .expect("error event");

    assert!(
        delta_index < end_index && end_index < error_index,
        "pending tool-call state must flush before Error on unexpected EOF: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AssistantMessageEvent::Done { .. })),
        "transport EOF without [DONE] must not complete normally"
    );
    match &events[error_index] {
        AssistantMessageEvent::Error { error_kind, .. } => {
            assert_eq!(*error_kind, Some(swink_agent::StreamErrorKind::Network));
        }
        event => panic!("expected network error, got {event:?}"),
    }
}

#[test]
fn done_sentinel_preserves_accumulated_stop_reason() {
    let mut state = OaiSseStreamState {
        stop_reason: Some(StopReason::ToolUse),
        ..Default::default()
    };
    state.tool_calls.insert(
        0,
        OaiToolCallEntry {
            id: "call_1".into(),
            name: Some("read_file".into()),
            arguments: r#"{"path":"foo.rs"}"#.into(),
            content_index: None,
        },
    );

    let events = state.emit_done_from_done_sentinel();
    let delta_index = events
        .iter()
        .position(|event| matches!(event, AssistantMessageEvent::ToolCallDelta { .. }))
        .expect("final tool delta");
    let end_index = events
        .iter()
        .position(|event| matches!(event, AssistantMessageEvent::ToolCallEnd { .. }))
        .expect("tool call end");
    let done_index = events
        .iter()
        .position(|event| matches!(event, AssistantMessageEvent::Done { .. }))
        .expect("done event");

    assert!(
        delta_index < end_index && end_index < done_index,
        "pending tool-call state must flush before [DONE] completion: {events:?}"
    );
    match &events[done_index] {
        AssistantMessageEvent::Done { stop_reason, .. } => {
            assert_eq!(*stop_reason, StopReason::ToolUse);
        }
        event => panic!("expected done event, got {event:?}"),
    }
}

#[test]
fn done_sentinel_reports_protocol_error_for_pending_tool_call_without_name() {
    let mut state = OaiSseStreamState {
        stop_reason: Some(StopReason::ToolUse),
        ..Default::default()
    };
    state.tool_calls.insert(
        0,
        OaiToolCallEntry {
            id: "call_1".into(),
            name: None,
            arguments: r#"{"path":"foo.rs"}"#.into(),
            content_index: None,
        },
    );

    let events = state.emit_done_from_done_sentinel();

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AssistantMessageEvent::ToolCallStart { .. })),
        "terminal drain must not synthesize nameless ToolCallStart events: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AssistantMessageEvent::Done { .. })),
        "nameless terminal tool calls must fail the stream instead of completing normally: {events:?}"
    );
    match events.last() {
        Some(AssistantMessageEvent::Error {
            error_message,
            error_kind,
            ..
        }) => {
            assert!(
                error_kind.is_none(),
                "protocol errors must not be retryable"
            );
            assert!(
                error_message.contains("missing function name"),
                "error should explain the terminal protocol fault: {error_message}"
            );
        }
        other => panic!("expected terminal protocol error, got {other:?}"),
    }
}
