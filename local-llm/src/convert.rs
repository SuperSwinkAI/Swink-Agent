//! Message conversion from swink-agent types to chat message pairs.
//!
//! Implements core's [`MessageConverter`] trait so the shared
//! [`convert_messages`](swink_agent::convert_messages) function handles
//! iteration / pattern-matching, while this module supplies the
//! role+content construction for llama.cpp chat templates.

use swink_agent::{
    AgentContext, AssistantMessage, ContentBlock, MessageConverter, ToolResultMessage, UserMessage,
    convert_messages,
};

use crate::model::ModelConfig;

// ─── Intermediate message type ──────────────────────────────────────────────

/// A role + content pair for building chat template input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalMessage {
    pub role: String,
    pub content: String,
}

// ─── MessageConverter impl ──────────────────────────────────────────────────

struct LocalConverter;

impl MessageConverter for LocalConverter {
    type Message = LocalMessage;

    fn system_message(system_prompt: &str) -> Option<Self::Message> {
        Some(LocalMessage {
            role: "system".to_string(),
            content: system_prompt.to_string(),
        })
    }

    fn user_message(user: &UserMessage) -> Self::Message {
        LocalMessage {
            role: "user".to_string(),
            content: ContentBlock::extract_text(&user.content),
        }
    }

    fn assistant_message(assistant: &AssistantMessage) -> Self::Message {
        LocalMessage {
            role: "assistant".to_string(),
            content: format_assistant_content(&assistant.content),
        }
    }

    fn tool_result_message(result: &ToolResultMessage) -> Self::Message {
        let text = ContentBlock::extract_text(&result.content);
        let content = format!("[tool_call_id: {}]\n{text}", result.tool_call_id);
        LocalMessage {
            role: "tool".to_string(),
            content,
        }
    }
}

fn format_assistant_content(blocks: &[ContentBlock]) -> String {
    let mut content = String::new();

    for block in blocks {
        match block {
            ContentBlock::Text { text } => content.push_str(text),
            ContentBlock::ToolCall {
                id,
                name,
                arguments,
                partial_json,
            } => {
                if !content.is_empty() && !content.ends_with('\n') {
                    content.push('\n');
                }
                let rendered_arguments;
                let arguments_text = if let Some(partial_json) = partial_json.as_deref() {
                    partial_json
                } else {
                    rendered_arguments = arguments.to_string();
                    rendered_arguments.as_str()
                };
                content.push_str("[tool_call_id: ");
                content.push_str(id);
                content.push_str("]\ncall:");
                content.push_str(name);
                content.push_str(arguments_text);
            }
            _ => {}
        }
    }

    content
}

// ─── Gemma 4 converter ──────────────────────────────────────────────────────

#[cfg(feature = "gemma4")]
struct Gemma4LocalConverter;

#[cfg(feature = "gemma4")]
impl MessageConverter for Gemma4LocalConverter {
    type Message = LocalMessage;

    fn system_message(system_prompt: &str) -> Option<Self::Message> {
        LocalConverter::system_message(system_prompt)
    }

    fn user_message(user: &UserMessage) -> Self::Message {
        LocalConverter::user_message(user)
    }

    fn assistant_message(assistant: &AssistantMessage) -> Self::Message {
        LocalConverter::assistant_message(assistant)
    }

    fn tool_result_message(result: &ToolResultMessage) -> Self::Message {
        let text = ContentBlock::extract_text(&result.content);
        LocalMessage {
            role: "tool".to_string(),
            content: format!(
                "<|tool_result>{}\n{text}<tool_result|>",
                result.tool_call_id
            ),
        }
    }
}

// ─── Public API ─────────────────────────────────────────────────────────────

/// Convert an [`AgentContext`] into a list of role+content message pairs.
///
/// When `config` identifies a Gemma 4 model and `thinking_enabled` is `true`,
/// prepends `<|think|>\n` to the system prompt to activate thinking mode.
pub fn convert_context_messages(
    context: &AgentContext,
    config: &ModelConfig,
    thinking_enabled: bool,
) -> Vec<LocalMessage> {
    let system_prompt = inject_think_token(&context.system_prompt, config, thinking_enabled);

    #[cfg(feature = "gemma4")]
    if config.is_gemma4() {
        return convert_messages::<Gemma4LocalConverter>(&context.messages, &system_prompt);
    }

    convert_messages::<LocalConverter>(&context.messages, &system_prompt)
}

/// Format `LocalMessage`s into a Gemma 4 prompt string directly.
///
/// Bypasses `llama_chat_apply_template` because Gemma 4's GGUF-embedded Jinja
/// template uses features (namespace, dictsort, get) that llama.cpp's template
/// engine cannot render (returns FFI error -1).
///
/// Format: `<|turn>{role}\n{content}<turn|>\n` per message, with "assistant"
/// mapped to "model". Ends with `<|turn>model\n` for generation.
#[cfg(feature = "gemma4")]
pub fn format_gemma4_prompt(messages: &[LocalMessage]) -> String {
    let mut prompt = String::new();
    for msg in messages {
        let role = if msg.role == "assistant" {
            "model"
        } else {
            &msg.role
        };
        prompt.push_str("<|turn>");
        prompt.push_str(role);
        prompt.push('\n');
        prompt.push_str(&msg.content);
        prompt.push_str("<turn|>\n");
    }
    // Add generation prompt
    prompt.push_str("<|turn>model\n");
    prompt
}

fn inject_think_token(system_prompt: &str, config: &ModelConfig, thinking_enabled: bool) -> String {
    #[cfg(feature = "gemma4")]
    if config.is_gemma4() && thinking_enabled {
        return format!("<|think|>\n{system_prompt}");
    }

    let _ = (config, thinking_enabled);
    system_prompt.to_string()
}

#[cfg(test)]
#[path = "convert_tests.rs"]
mod tests;
