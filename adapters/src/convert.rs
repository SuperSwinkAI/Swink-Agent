//! Shared message conversion utilities for LLM adapters.
//!
//! Provides a [`MessageConverter`] trait that each adapter implements to supply
//! format-specific conversion logic, while the generic [`convert_messages`]
//! function handles the common iteration and pattern matching over
//! [`AgentMessage`] / [`LlmMessage`] variants.

use agent_harness::stream::AssistantMessageEvent;
use agent_harness::types::{
    AgentMessage, AssistantMessage, LlmMessage, StopReason, ToolResultMessage, UserMessage,
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

    if !system_prompt.is_empty() {
        if let Some(sys) = C::system_message(system_prompt) {
            result.push(sys);
        }
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

// ─── Shared error_event helper ──────────────────────────────────────────────

/// Create an error event.
///
/// All adapters use the same shape for stream-level errors, so this lives here
/// to avoid duplication.
pub fn error_event(message: &str) -> AssistantMessageEvent {
    AssistantMessageEvent::Error {
        stop_reason: StopReason::Error,
        error_message: message.to_string(),
        usage: None,
    }
}
