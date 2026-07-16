//! Interrupt state persistence for resuming interrupted agent sessions.
//!
//! When an agent is interrupted (e.g., at a tool approval gate or by
//! cancellation), the [`InterruptState`] captures enough context to resume
//! exactly where the agent left off after a process restart.

use serde::{Deserialize, Serialize};
use swink_agent::{LlmMessage, ModelSpec};

/// A tool call awaiting approval at the time of interruption.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingToolCall {
    /// Unique identifier for this tool call invocation.
    pub tool_call_id: String,
    /// Name of the tool being called.
    pub tool_name: String,
    /// Tool arguments passed to the tool.
    pub arguments: serde_json::Value,
}

impl PendingToolCall {
    /// Creates a new pending tool call record.
    #[must_use]
    pub fn new(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            arguments,
        }
    }
}

/// Snapshot of agent state at the point of interruption.
///
/// Persisted as `{session_id}.interrupt.json` alongside the session JSONL file.
/// File existence indicates an active interrupt; deletion means the agent has
/// resumed.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterruptState {
    /// Unix timestamp of when the interrupt occurred.
    pub interrupted_at: u64,
    /// Tool calls awaiting approval at interrupt time.
    pub pending_tool_calls: Vec<PendingToolCall>,
    /// Conversation context frozen at the interrupt point.
    pub context_snapshot: Vec<LlmMessage>,
    /// Active system prompt at interrupt time.
    pub system_prompt: String,
    /// Active model at interrupt time.
    pub model: ModelSpec,
}

impl InterruptState {
    /// Creates a new interrupt state snapshot.
    #[must_use]
    pub fn new(
        interrupted_at: u64,
        pending_tool_calls: Vec<PendingToolCall>,
        context_snapshot: Vec<LlmMessage>,
        system_prompt: impl Into<String>,
        model: ModelSpec,
    ) -> Self {
        Self {
            interrupted_at,
            pending_tool_calls,
            context_snapshot,
            system_prompt: system_prompt.into(),
            model,
        }
    }
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<InterruptState>();
    assert_send_sync::<PendingToolCall>();
};

#[cfg(test)]
mod tests {
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
}
