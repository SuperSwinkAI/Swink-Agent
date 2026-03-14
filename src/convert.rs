//! Shared message conversion utilities for LLM adapters.
//!
//! Provides a [`MessageConverter`] trait that each adapter implements to supply
//! format-specific conversion logic, while the generic [`convert_messages`]
//! function handles the common iteration and pattern matching over
//! [`AgentMessage`] / [`LlmMessage`] variants.

use std::sync::Arc;

use serde_json::Value;

use crate::AgentTool;
use crate::types::{
    AgentMessage, AssistantMessage, LlmMessage, ToolResultMessage, UserMessage,
};

// ─── MessageConverter trait ─────────────────────────────────────────────────

/// Callbacks for provider-specific message conversion.
///
/// Each adapter implements this trait so the shared [`convert_messages`]
/// function can build the provider's message list without duplicating the
/// iteration / pattern-matching boilerplate.
pub trait MessageConverter {
    /// The provider-specific message type (e.g. `OllamaMessage`, `OpenAiMessage`).
    type Message;

    /// Optionally produce a system message from the system prompt.
    /// Return `None` if the provider handles system prompts out-of-band
    /// (e.g. Anthropic uses a top-level `system` field).
    fn system_message(system_prompt: &str) -> Option<Self::Message>;

    /// Convert a user message.
    fn user_message(user: &UserMessage) -> Self::Message;

    /// Convert an assistant message.
    fn assistant_message(assistant: &AssistantMessage) -> Self::Message;

    /// Convert a tool-result message.
    fn tool_result_message(result: &ToolResultMessage) -> Self::Message;
}

/// Generic message conversion that iterates [`AgentMessage`] values, skips
/// non-LLM variants, and delegates to the [`MessageConverter`] implementation
/// for format-specific construction.
pub fn convert_messages<C: MessageConverter>(
    messages: &[AgentMessage],
    system_prompt: &str,
) -> Vec<C::Message> {
    let mut result = Vec::new();

    if !system_prompt.is_empty()
        && let Some(sys) = C::system_message(system_prompt)
    {
        result.push(sys);
    }

    for msg in messages {
        let AgentMessage::Llm(llm) = msg else {
            continue;
        };
        match llm {
            LlmMessage::User(user) => result.push(C::user_message(user)),
            LlmMessage::Assistant(assistant) => result.push(C::assistant_message(assistant)),
            LlmMessage::ToolResult(tool_result) => {
                result.push(C::tool_result_message(tool_result));
            }
        }
    }

    result
}

// ─── Tool schema extraction ─────────────────────────────────────────────────

/// Common tool metadata extracted from [`AgentTool`] instances.
///
/// Used by adapters to avoid duplicating the `name` / `description` /
/// `parameters` mapping before wrapping in provider-specific types.
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// Extract tool metadata from a slice of [`AgentTool`] trait objects.
pub fn extract_tool_schemas(tools: &[Arc<dyn AgentTool>]) -> Vec<ToolSchema> {
    tools
        .iter()
        .map(|t| ToolSchema {
            name: t.name().to_string(),
            description: t.description().to_string(),
            parameters: t.parameters_schema().clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        AgentMessage, AssistantMessage, ContentBlock, Cost, LlmMessage, StopReason,
        ToolResultMessage, Usage, UserMessage,
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
            timestamp: 0,
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
}
