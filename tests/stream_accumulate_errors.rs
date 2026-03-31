//! Tests for untested error paths in `accumulate_message`.

use swink_agent::{AssistantMessageEvent, accumulate_message};

// ── 1. TextEnd without prior TextStart (no blocks exist at that index) ──

#[test]
fn text_end_before_start_event() {
    let events = vec![AssistantMessageEvent::TextEnd { content_index: 0 }];
    let err = accumulate_message(events, "p", "m").unwrap_err();
    assert!(err.contains("before Start"), "got: {err}");
}

#[test]
fn text_end_invalid_content_index() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::TextEnd { content_index: 0 },
    ];
    let err = accumulate_message(events, "p", "m").unwrap_err();
    assert!(
        err.contains("invalid content_index"),
        "got: {err}"
    );
}

// ── 3. TextEnd on non-Text block (e.g. a ToolCall block) ──

#[test]
fn text_end_on_non_text_block() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            id: "tc_0".into(),
            name: "tool".into(),
        },
        AssistantMessageEvent::TextEnd { content_index: 0 },
    ];
    let err = accumulate_message(events, "p", "m").unwrap_err();
    assert!(err.contains("not Text"), "got: {err}");
}

// ── 4. ThinkingStart before Start event ──

#[test]
fn thinking_start_before_start_event() {
    let events = vec![AssistantMessageEvent::ThinkingStart { content_index: 0 }];
    let err = accumulate_message(events, "p", "m").unwrap_err();
    assert!(err.contains("before Start"), "got: {err}");
}

// ── 5. ThinkingDelta on wrong block type (ToolCall block) ──

#[test]
fn thinking_delta_on_tool_call_block() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            id: "tc_0".into(),
            name: "tool".into(),
        },
        AssistantMessageEvent::ThinkingDelta {
            content_index: 0,
            delta: "oops".into(),
        },
    ];
    let err = accumulate_message(events, "p", "m").unwrap_err();
    assert!(err.contains("not Thinking"), "got: {err}");
}

// ── 6. ThinkingEnd on wrong block type (Text block) ──

#[test]
fn thinking_end_on_text_block() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::TextStart { content_index: 0 },
        AssistantMessageEvent::ThinkingEnd {
            content_index: 0,
            signature: None,
        },
    ];
    let err = accumulate_message(events, "p", "m").unwrap_err();
    assert!(err.contains("not Thinking"), "got: {err}");
}

#[test]
fn thinking_end_on_tool_call_block() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            id: "tc_0".into(),
            name: "tool".into(),
        },
        AssistantMessageEvent::ThinkingEnd {
            content_index: 0,
            signature: Some("sig".into()),
        },
    ];
    let err = accumulate_message(events, "p", "m").unwrap_err();
    assert!(err.contains("not Thinking"), "got: {err}");
}

// ── 7. ToolCallStart before Start event ──

#[test]
fn tool_call_start_before_start_event() {
    let events = vec![AssistantMessageEvent::ToolCallStart {
        content_index: 0,
        id: "tc_0".into(),
        name: "tool".into(),
    }];
    let err = accumulate_message(events, "p", "m").unwrap_err();
    assert!(err.contains("before Start"), "got: {err}");
}

// ── 8. ToolCallDelta with partial_json already consumed ──

#[test]
fn tool_call_delta_after_partial_json_consumed() {
    // ToolCallEnd consumes partial_json; a subsequent delta should fail.
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            id: "tc_0".into(),
            name: "tool".into(),
        },
        AssistantMessageEvent::ToolCallEnd { content_index: 0 },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            delta: "more".into(),
        },
    ];
    let err = accumulate_message(events, "p", "m").unwrap_err();
    assert!(err.contains("partial_json already consumed"), "got: {err}");
}

// ── 9. ToolCallDelta on wrong block type (Text block) ──

#[test]
fn tool_call_delta_on_text_block() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::TextStart { content_index: 0 },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            delta: "oops".into(),
        },
    ];
    let err = accumulate_message(events, "p", "m").unwrap_err();
    assert!(err.contains("not ToolCall"), "got: {err}");
}

// ── 10. ToolCallEnd with partial_json already consumed ──

#[test]
fn tool_call_end_after_partial_json_consumed() {
    // End the tool call once (consumes partial_json), then end again.
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            id: "tc_0".into(),
            name: "tool".into(),
        },
        AssistantMessageEvent::ToolCallEnd { content_index: 0 },
        AssistantMessageEvent::ToolCallEnd { content_index: 0 },
    ];
    let err = accumulate_message(events, "p", "m").unwrap_err();
    assert!(err.contains("partial_json already consumed"), "got: {err}");
}

// ── 11. ToolCallEnd with invalid JSON in partial_json ──

#[test]
fn tool_call_end_invalid_json() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            id: "tc_0".into(),
            name: "tool".into(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            delta: "not valid json{{{".into(),
        },
        AssistantMessageEvent::ToolCallEnd { content_index: 0 },
    ];
    let err = accumulate_message(events, "p", "m").unwrap_err();
    assert!(err.contains("failed to parse arguments JSON"), "got: {err}");
}

// ── 12. ToolCallEnd on wrong block type (Text block) ──

#[test]
fn tool_call_end_on_text_block() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::TextStart { content_index: 0 },
        AssistantMessageEvent::ToolCallEnd { content_index: 0 },
    ];
    let err = accumulate_message(events, "p", "m").unwrap_err();
    assert!(err.contains("not ToolCall"), "got: {err}");
}

#[test]
fn tool_call_end_on_thinking_block() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ThinkingStart { content_index: 0 },
        AssistantMessageEvent::ToolCallEnd { content_index: 0 },
    ];
    let err = accumulate_message(events, "p", "m").unwrap_err();
    assert!(err.contains("not ToolCall"), "got: {err}");
}
