//! Tests for `interrupt`.
#![cfg(test)]

use super::*;

#[test]
fn serde_roundtrip() {
    let state = InterruptState {
        interrupted_at: 1_710_500_000,
        pending_tool_calls: vec![
            PendingToolCall {
                tool_call_id: "tc_1".to_string(),
                tool_name: "bash".to_string(),
                arguments: serde_json::json!({"command": "ls"}),
            },
            PendingToolCall {
                tool_call_id: "tc_2".to_string(),
                tool_name: "read_file".to_string(),
                arguments: serde_json::json!({"path": "/tmp/foo.txt"}),
            },
        ],
        context_snapshot: vec![],
        system_prompt: "You are a helpful assistant.".to_string(),
        model: ModelSpec::new("openai", "gpt-4"),
    };

    let json = serde_json::to_string(&state).unwrap();
    let deserialized: InterruptState = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.interrupted_at, state.interrupted_at);
    assert_eq!(deserialized.pending_tool_calls, state.pending_tool_calls);
    assert_eq!(deserialized.system_prompt, state.system_prompt);
    assert_eq!(
        deserialized.context_snapshot.len(),
        state.context_snapshot.len()
    );
}
