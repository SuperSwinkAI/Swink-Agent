//! Display-ready message types for frontend consumption.
//!
//! Provides a core display representation that any frontend (TUI, GUI, web)
//! can wrap with UI-specific fields (collapse state, scroll position, etc.).

use crate::types::{ContentBlock, LlmMessage, StopReason};

/// Role of a message for display styling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayRole {
    User,
    Assistant,
    ToolResult,
    Error,
    System,
}

/// A message converted to a frontend-friendly format.
///
/// Contains the essential display data extracted from [`LlmMessage`].
/// Frontend implementations can wrap this with additional UI-specific
/// fields (collapse state, scroll position, etc.).
#[derive(Debug, Clone)]
pub struct CoreDisplayMessage {
    pub role: DisplayRole,
    pub content: String,
    pub thinking: Option<String>,
}

/// Convert message types into display-ready representations.
pub trait IntoDisplayMessages {
    fn to_display_messages(&self) -> Vec<CoreDisplayMessage>;
}

impl IntoDisplayMessages for LlmMessage {
    fn to_display_messages(&self) -> Vec<CoreDisplayMessage> {
        match self {
            Self::User(user) => {
                vec![CoreDisplayMessage {
                    role: DisplayRole::User,
                    content: ContentBlock::extract_text(&user.content),
                    thinking: None,
                }]
            }
            Self::Assistant(assistant) => {
                let mut text_parts = Vec::new();
                let mut thinking_parts = Vec::new();
                for block in &assistant.content {
                    match block {
                        ContentBlock::Text { text } => text_parts.push(text.as_str()),
                        ContentBlock::Thinking { thinking, .. } => {
                            thinking_parts.push(thinking.as_str());
                        }
                        _ => {}
                    }
                }

                let content = if !text_parts.is_empty() {
                    text_parts.join("")
                } else if assistant.stop_reason == StopReason::Error {
                    assistant.error_message.clone().unwrap_or_default()
                } else {
                    String::new()
                };

                let thinking = if thinking_parts.is_empty() {
                    None
                } else {
                    Some(thinking_parts.join(""))
                };

                let role = if assistant.stop_reason == StopReason::Error {
                    DisplayRole::Error
                } else {
                    DisplayRole::Assistant
                };

                vec![CoreDisplayMessage {
                    role,
                    content,
                    thinking,
                }]
            }
            Self::ToolResult(tool_result) => {
                let content = ContentBlock::extract_text(&tool_result.content);
                if content.is_empty() {
                    return vec![];
                }
                let role = if tool_result.is_error {
                    DisplayRole::Error
                } else {
                    DisplayRole::ToolResult
                };
                vec![CoreDisplayMessage {
                    role,
                    content,
                    thinking: None,
                }]
            }
        }
    }
}

impl IntoDisplayMessages for [LlmMessage] {
    fn to_display_messages(&self) -> Vec<CoreDisplayMessage> {
        self.iter()
            .flat_map(IntoDisplayMessages::to_display_messages)
            .collect()
    }
}

impl IntoDisplayMessages for Vec<LlmMessage> {
    fn to_display_messages(&self) -> Vec<CoreDisplayMessage> {
        self.as_slice().to_display_messages()
    }
}

#[cfg(test)]
mod tests {
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
}
