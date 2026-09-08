//! Tests for `display`.
#![cfg(test)]

use super::*;
use crate::types::{AssistantMessage, Cost, Usage, UserMessage};

#[test]
fn user_message_to_display() {
    let msg = LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: "hello".to_string(),
        }],
        timestamp: 0,
        cache_hint: None,
    });
    let display = msg.to_display_messages();
    assert_eq!(display.len(), 1);
    assert_eq!(display[0].role, DisplayRole::User);
    assert_eq!(display[0].content, "hello");
    assert!(display[0].thinking.is_none());
}

#[test]
fn assistant_message_with_thinking() {
    let msg = LlmMessage::Assistant(AssistantMessage {
        content: vec![
            ContentBlock::Thinking {
                thinking: "reasoning".to_string(),
                signature: None,
            },
            ContentBlock::Text {
                text: "answer".to_string(),
            },
        ],
        provider: String::new(),
        model_id: String::new(),
        usage: Usage::default(),
        cost: Cost::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        error_kind: None,
        timestamp: 0,
        cache_hint: None,
    });
    let display = msg.to_display_messages();
    assert_eq!(display.len(), 1);
    assert_eq!(display[0].role, DisplayRole::Assistant);
    assert_eq!(display[0].content, "answer");
    assert_eq!(display[0].thinking.as_deref(), Some("reasoning"));
}

#[test]
fn assistant_error_message() {
    let msg = LlmMessage::Assistant(AssistantMessage {
        content: vec![],
        provider: String::new(),
        model_id: String::new(),
        usage: Usage::default(),
        cost: Cost::default(),
        stop_reason: StopReason::Error,
        error_message: Some("something broke".to_string()),
        error_kind: None,
        timestamp: 0,
        cache_hint: None,
    });
    let display = msg.to_display_messages();
    assert_eq!(display.len(), 1);
    assert_eq!(display[0].role, DisplayRole::Error);
    assert_eq!(display[0].content, "something broke");
}

#[test]
fn empty_tool_result_produces_no_messages() {
    let msg = LlmMessage::ToolResult(crate::types::ToolResultMessage {
        tool_call_id: "tc1".to_string(),
        content: vec![],
        is_error: false,
        timestamp: 0,
        details: serde_json::Value::Null,
        cache_hint: None,
    });
    let display = msg.to_display_messages();
    assert!(display.is_empty());
}
