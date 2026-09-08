//! Tests for `convert`.
#![cfg(test)]

use super::*;
use crate::types::{
    AgentMessage, AssistantMessage, ContentBlock, Cost, LlmMessage, StopReason, ToolResultMessage,
    Usage, UserMessage,
};

// ── Test converter ──────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
struct TestMessage {
    role: String,
    content: String,
}

struct TestConverter;

impl MessageConverter for TestConverter {
    type Message = TestMessage;

    fn system_message(prompt: &str) -> Option<Self::Message> {
        Some(TestMessage {
            role: "system".to_string(),
            content: prompt.to_string(),
        })
    }

    fn user_message(msg: &UserMessage) -> Self::Message {
        let text = ContentBlock::extract_text(&msg.content);
        TestMessage {
            role: "user".to_string(),
            content: text,
        }
    }

    fn assistant_message(msg: &AssistantMessage) -> Self::Message {
        let text = ContentBlock::extract_text(&msg.content);
        TestMessage {
            role: "assistant".to_string(),
            content: text,
        }
    }

    fn tool_result_message(msg: &ToolResultMessage) -> Self::Message {
        let text = ContentBlock::extract_text(&msg.content);
        TestMessage {
            role: "tool".to_string(),
            content: text,
        }
    }
}

/// A converter that returns `None` for system messages.
struct NoSystemConverter;

impl MessageConverter for NoSystemConverter {
    type Message = TestMessage;

    fn system_message(_prompt: &str) -> Option<Self::Message> {
        None
    }

    fn user_message(msg: &UserMessage) -> Self::Message {
        TestConverter::user_message(msg)
    }

    fn assistant_message(msg: &AssistantMessage) -> Self::Message {
        TestConverter::assistant_message(msg)
    }

    fn tool_result_message(msg: &ToolResultMessage) -> Self::Message {
        TestConverter::tool_result_message(msg)
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

fn make_user(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        timestamp: 0,
        cache_hint: None,
    }))
}

fn make_assistant(text: &str) -> AgentMessage {
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
        error_kind: None,
        timestamp: 0,
        cache_hint: None,
    }))
}

fn make_tool_result(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::ToolResult(ToolResultMessage {
        tool_call_id: "tc1".to_string(),
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        is_error: false,
        timestamp: 0,
        details: serde_json::Value::Null,
        cache_hint: None,
    }))
}

// ── convert_messages tests ──────────────────────────────────────────

#[test]
fn convert_empty_messages_no_system() {
    let result = convert_messages::<TestConverter>(&[], "");
    assert!(result.is_empty());
}

#[test]
fn convert_system_prompt_only() {
    let result = convert_messages::<TestConverter>(&[], "test prompt");
    assert_eq!(result.len(), 1);
    assert_eq!(
        result[0],
        TestMessage {
            role: "system".to_string(),
            content: "test prompt".to_string(),
        }
    );
}

#[test]
fn convert_user_message_included() {
    let messages = vec![make_user("hello")];
    let result = convert_messages::<TestConverter>(&messages, "");
    assert_eq!(result.len(), 1);
    assert_eq!(
        result[0],
        TestMessage {
            role: "user".to_string(),
            content: "hello".to_string(),
        }
    );
}

#[test]
fn convert_assistant_message_included() {
    let messages = vec![make_assistant("hi there")];
    let result = convert_messages::<TestConverter>(&messages, "");
    assert_eq!(result.len(), 1);
    assert_eq!(
        result[0],
        TestMessage {
            role: "assistant".to_string(),
            content: "hi there".to_string(),
        }
    );
}

#[test]
fn convert_tool_result_message_included() {
    let messages = vec![make_tool_result("result data")];
    let result = convert_messages::<TestConverter>(&messages, "");
    assert_eq!(result.len(), 1);
    assert_eq!(
        result[0],
        TestMessage {
            role: "tool".to_string(),
            content: "result data".to_string(),
        }
    );
}

#[test]
fn convert_mixed_messages() {
    let messages = vec![
        make_user("question"),
        make_assistant("answer"),
        make_tool_result("tool output"),
    ];
    let result = convert_messages::<TestConverter>(&messages, "sys");
    assert_eq!(result.len(), 4);
    assert_eq!(result[0].role, "system");
    assert_eq!(result[1].role, "user");
    assert_eq!(result[2].role, "assistant");
    assert_eq!(result[3].role, "tool");
}

#[test]
fn convert_skips_custom_messages() {
    use std::any::Any;

    #[derive(Debug)]
    struct MyCustom;
    impl crate::types::CustomMessage for MyCustom {
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    let messages = vec![
        make_user("before"),
        AgentMessage::Custom(Box::new(MyCustom)),
        make_user("after"),
    ];
    let result = convert_messages::<TestConverter>(&messages, "");
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].content, "before");
    assert_eq!(result[1].content, "after");
}

#[test]
fn convert_no_system_when_converter_returns_none() {
    let messages = vec![make_user("hello")];
    let result = convert_messages::<NoSystemConverter>(&messages, "ignored prompt");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].role, "user");
}
