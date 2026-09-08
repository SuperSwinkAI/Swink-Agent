//! Tests for `trajectory`.
#![cfg(test)]

use super::*;

#[test]
fn finalize_tool_calls_backfills_missing_starts_from_assistant_message() {
    let assistant_message = AssistantMessage::new(
        vec![ContentBlock::ToolCall {
            id: "call_1".to_string(),
            name: "write_file".to_string(),
            arguments: serde_json::json!({"path": "notes.txt"}),
            partial_json: None,
        }],
        "test".to_string(),
        "test-model".to_string(),
    )
    .with_stop_reason(StopReason::ToolUse)
    .with_timestamp(0);

    let tool_calls = finalize_tool_calls(Vec::new(), &assistant_message);

    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].id, "call_1");
    assert_eq!(tool_calls[0].name, "write_file");
    assert_eq!(
        tool_calls[0].arguments,
        serde_json::json!({"path": "notes.txt"})
    );
}

#[test]
fn finalize_tool_calls_prefers_execution_start_arguments_when_present() {
    let assistant_message = AssistantMessage::new(
        vec![ContentBlock::ToolCall {
            id: "call_1".to_string(),
            name: "write_file".to_string(),
            arguments: serde_json::json!({"path": "original.txt"}),
            partial_json: None,
        }],
        "test".to_string(),
        "test-model".to_string(),
    )
    .with_stop_reason(StopReason::ToolUse)
    .with_timestamp(0);
    let observed_tool_calls = vec![RecordedToolCall {
        id: "call_1".to_string(),
        name: "write_file".to_string(),
        arguments: serde_json::json!({"path": "rewritten.txt"}),
    }];

    let tool_calls = finalize_tool_calls(observed_tool_calls, &assistant_message);

    assert_eq!(tool_calls.len(), 1);
    assert_eq!(
        tool_calls[0].arguments,
        serde_json::json!({"path": "rewritten.txt"})
    );
}
