//! Test helper functions for building common message types.
//!
//! Gated behind the `test-helpers` feature so downstream crates can
//! reuse them in their own test suites without duplicating constructors.

use crate::types::{
    AgentMessage, AssistantMessage, ContentBlock, Cost, LlmMessage, StopReason, ToolResultMessage,
    Usage, UserMessage,
};

/// Build a single [`AgentMessage::Llm`] user message with the given text.
pub fn user_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        timestamp: 0,
    }))
}

/// Build a single [`AgentMessage::Llm`] assistant message with the given text.
pub fn assistant_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        provider: String::new(),
        model_id: String::new(),
        usage: Usage::default(),
        cost: Cost::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    }))
}

/// Build a single [`AgentMessage::Llm`] tool-result message.
pub fn tool_result_msg(id: &str, text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::ToolResult(ToolResultMessage {
        tool_call_id: id.to_string(),
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        is_error: false,
        timestamp: 0,
        details: serde_json::Value::Null,
    }))
}
