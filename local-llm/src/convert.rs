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
mod tests {
    use super::*;

    use serde_json::json;
    use swink_agent::testing::{assistant_msg, tool_result_msg, user_msg};
    use swink_agent::{
        AgentMessage, AssistantMessage, ContentBlock, Cost, LlmMessage, StopReason,
        ToolResultMessage, Usage, UserMessage,
    };

    fn make_context(system: &str, messages: Vec<AgentMessage>) -> AgentContext {
        AgentContext {
            system_prompt: system.to_string(),
            messages,
            tools: vec![],
        }
    }

    fn smollm_config() -> ModelConfig {
        ModelConfig {
            repo_id: "unsloth/SmolLM3-3B-GGUF".to_string(),
            ..ModelConfig::default()
        }
    }

    #[test]
    fn empty_context_produces_system_only() {
        let ctx = make_context("sys", vec![]);
        let msgs = convert_context_messages(&ctx, &smollm_config(), false);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "system");
    }

    #[test]
    fn system_prompt_is_included() {
        let ctx = make_context("You are helpful.", vec![]);
        let msgs = convert_context_messages(&ctx, &smollm_config(), false);
        assert_eq!(msgs[0].content, "You are helpful.");
    }

    #[test]
    fn mixed_messages_converted() {
        let ctx = make_context(
            "sys",
            vec![
                user_msg("hello"),
                assistant_msg("hi"),
                tool_result_msg("tc1", "result"),
            ],
        );
        let msgs = convert_context_messages(&ctx, &smollm_config(), false);
        assert_eq!(msgs.len(), 4); // system + 3 messages
        assert_eq!(msgs[1].role, "user");
        assert_eq!(msgs[2].role, "assistant");
        assert_eq!(msgs[3].role, "tool");
    }

    #[test]
    fn custom_messages_skipped() {
        use std::any::Any;

        #[derive(Debug)]
        struct Custom;
        impl swink_agent::CustomMessage for Custom {
            fn as_any(&self) -> &dyn Any {
                self
            }
        }

        let ctx = make_context(
            "sys",
            vec![
                user_msg("before"),
                AgentMessage::Custom(Box::new(Custom)),
                user_msg("after"),
            ],
        );
        let msgs = convert_context_messages(&ctx, &smollm_config(), false);
        // system + 2 user messages (custom skipped)
        assert_eq!(msgs.len(), 3);
    }

    #[test]
    fn empty_assistant_message_no_panic() {
        let msg = AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
            content: vec![],
            provider: String::new(),
            model_id: String::new(),
            usage: Usage::default(),
            cost: Cost::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            error_kind: None,
            timestamp: 0,
            cache_hint: None,
        }));
        let ctx = make_context("sys", vec![msg]);
        let msgs = convert_context_messages(&ctx, &smollm_config(), false);
        assert_eq!(msgs[1].role, "assistant");
    }

    #[test]
    fn assistant_tool_calls_preserved() {
        let msg = AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
            content: vec![
                ContentBlock::Text {
                    text: "I need to inspect the file.".to_string(),
                },
                ContentBlock::ToolCall {
                    id: "tc-1".to_string(),
                    name: "read_file".to_string(),
                    arguments: json!({ "path": "Cargo.toml" }),
                    partial_json: None,
                },
            ],
            provider: String::new(),
            model_id: String::new(),
            usage: Usage::default(),
            cost: Cost::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            error_kind: None,
            timestamp: 0,
            cache_hint: None,
        }));
        let ctx = make_context("sys", vec![msg]);
        let msgs = convert_context_messages(&ctx, &smollm_config(), false);

        assert_eq!(
            msgs[1].content,
            "I need to inspect the file.\n[tool_call_id: tc-1]\ncall:read_file{\"path\":\"Cargo.toml\"}"
        );
    }

    #[test]
    fn tool_result_includes_call_id() {
        let ctx = make_context("sys", vec![tool_result_msg("tc-42", "file contents")]);
        let msgs = convert_context_messages(&ctx, &smollm_config(), false);
        assert!(msgs[1].content.contains("tc-42"));
        assert!(msgs[1].content.contains("file contents"));
    }

    #[test]
    fn multiple_content_blocks_concatenated() {
        let msg = AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![
                ContentBlock::Text {
                    text: "Hello ".to_string(),
                },
                ContentBlock::Text {
                    text: "world!".to_string(),
                },
            ],
            timestamp: 0,
            cache_hint: None,
        }));
        let ctx = make_context("sys", vec![msg]);
        let msgs = convert_context_messages(&ctx, &smollm_config(), false);
        assert_eq!(msgs[1].content, "Hello world!");
    }

    #[test]
    fn non_text_content_blocks_ignored() {
        let msg = AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![
                ContentBlock::Text {
                    text: "text part".to_string(),
                },
                ContentBlock::Thinking {
                    thinking: "internal thought".to_string(),
                    signature: None,
                },
                ContentBlock::ToolCall {
                    id: "tc-1".to_string(),
                    name: "bash".to_string(),
                    arguments: json!({}),
                    partial_json: None,
                },
            ],
            timestamp: 0,
            cache_hint: None,
        }));
        let ctx = make_context("sys", vec![msg]);
        let msgs = convert_context_messages(&ctx, &smollm_config(), false);
        assert_eq!(msgs[1].content, "text part");
    }

    #[test]
    fn tool_result_error_message_no_panic() {
        let msg = AgentMessage::Llm(LlmMessage::ToolResult(ToolResultMessage {
            tool_call_id: "tc-err".to_string(),
            content: vec![ContentBlock::Text {
                text: "error: command failed".to_string(),
            }],
            is_error: true,
            timestamp: 0,
            details: serde_json::Value::Null,
            cache_hint: None,
        }));
        let ctx = make_context("", vec![msg]);
        let _msgs = convert_context_messages(&ctx, &smollm_config(), false);
    }

    #[cfg(feature = "gemma4")]
    #[test]
    fn tool_result_formatting() {
        use swink_agent::ToolResultMessage;

        let result = ToolResultMessage {
            tool_call_id: "read_file".to_string(),
            content: vec![ContentBlock::Text {
                text: "file contents".to_string(),
            }],
            is_error: false,
            timestamp: 0,
            details: serde_json::Value::Null,
            cache_hint: None,
        };

        let msg = Gemma4LocalConverter::tool_result_message(&result);
        assert_eq!(
            msg.content,
            "<|tool_result>read_file\nfile contents<tool_result|>"
        );
    }

    #[cfg(feature = "gemma4")]
    mod think_token_tests {
        use super::*;

        fn gemma4_config() -> ModelConfig {
            ModelConfig {
                repo_id: "bartowski/google_gemma-4-E2B-it-GGUF".to_string(),
                ..ModelConfig::default()
            }
        }

        #[test]
        fn think_token_injected_for_gemma4() {
            let result = inject_think_token("You are helpful.", &gemma4_config(), true);
            assert!(result.starts_with("<|think|>\n"));
            assert!(result.contains("You are helpful."));
        }

        #[test]
        fn think_token_not_injected_for_smollm() {
            let result = inject_think_token("You are helpful.", &smollm_config(), true);
            assert!(!result.contains("<|think|>"));
            assert_eq!(result, "You are helpful.");
        }

        #[test]
        fn think_token_not_injected_when_thinking_disabled() {
            let result = inject_think_token("You are helpful.", &gemma4_config(), false);
            assert!(!result.contains("<|think|>"));
            assert_eq!(result, "You are helpful.");
        }
    }
}
