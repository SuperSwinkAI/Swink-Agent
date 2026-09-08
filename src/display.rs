//! Display-ready message types for frontend consumption.
//!
//! Provides a core display representation that any frontend (TUI, GUI, web)
//! can wrap with UI-specific fields (collapse state, scroll position, etc.).

use crate::types::{ContentBlock, LlmMessage, StopReason};

/// Role of a message for display styling.
#[non_exhaustive]
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
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct CoreDisplayMessage {
    pub role: DisplayRole,
    pub content: String,
    pub thinking: Option<String>,
}

impl CoreDisplayMessage {
    /// Create a new display message with no thinking content.
    #[must_use]
    pub fn new(role: DisplayRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            thinking: None,
        }
    }

    /// Attach thinking/reasoning content to this message.
    #[must_use]
    pub fn with_thinking(mut self, thinking: impl Into<String>) -> Self {
        self.thinking = Some(thinking.into());
        self
    }
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
#[path = "display_tests.rs"]
mod tests;
