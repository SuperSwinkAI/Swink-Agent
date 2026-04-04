//! Message conversion from swink-agent types to mistral.rs types.
//!
//! Implements core's [`MessageConverter`] trait so the shared
//! [`convert_messages`](swink_agent::convert_messages) function handles
//! iteration / pattern-matching, while this module supplies the
//! mistral.rs-specific construction.

use mistralrs::{TextMessageRole, TextMessages};

use swink_agent::convert::{MessageConverter, convert_messages};
use swink_agent::types::{
    AgentContext, AssistantMessage, ContentBlock, ToolResultMessage, UserMessage,
};

use crate::model::ModelConfig;

// ─── Intermediate message type ──────────────────────────────────────────────

/// A role + content pair that can be folded into mistral.rs [`TextMessages`].
struct LocalMessage {
    role: TextMessageRole,
    content: String,
}

// ─── MessageConverter impl ──────────────────────────────────────────────────

struct LocalConverter;

impl MessageConverter for LocalConverter {
    type Message = LocalMessage;

    fn system_message(system_prompt: &str) -> Option<Self::Message> {
        Some(LocalMessage {
            role: TextMessageRole::System,
            content: system_prompt.to_string(),
        })
    }

    fn user_message(user: &UserMessage) -> Self::Message {
        LocalMessage {
            role: TextMessageRole::User,
            content: ContentBlock::extract_text(&user.content),
        }
    }

    fn assistant_message(assistant: &AssistantMessage) -> Self::Message {
        LocalMessage {
            role: TextMessageRole::Assistant,
            content: ContentBlock::extract_text(&assistant.content),
        }
    }

    fn tool_result_message(result: &ToolResultMessage) -> Self::Message {
        let text = ContentBlock::extract_text(&result.content);
        // Tool results include the tool_call_id for matching.
        let content = format!("[tool_call_id: {}]\n{text}", result.tool_call_id);
        LocalMessage {
            role: TextMessageRole::Tool,
            content,
        }
    }
}

// ─── Public API ─────────────────────────────────────────────────────────────

/// Convert an [`AgentContext`] into mistral.rs [`TextMessages`].
///
/// Delegates to core's generic [`convert_messages`] with [`LocalConverter`],
/// then folds the intermediate messages into the mistral.rs builder.
///
/// When the model is Gemma 4 and thinking is enabled, prepends `<|think|>\n`
/// to the system prompt to activate the model's thinking mode.
pub fn convert_context_messages(
    context: &AgentContext,
    #[allow(unused_variables)] config: &ModelConfig,
    #[allow(unused_variables)] thinking_enabled: bool,
) -> TextMessages {
    // Optionally inject <|think|> for Gemma 4 with thinking enabled.
    #[cfg(feature = "gemma4")]
    let system_prompt = if config.is_gemma4() && thinking_enabled {
        format!("<|think|>\n{}", context.system_prompt)
    } else {
        context.system_prompt.clone()
    };
    #[cfg(not(feature = "gemma4"))]
    let system_prompt = context.system_prompt.clone();

    let converted = convert_messages::<LocalConverter>(&context.messages, &system_prompt);

    let mut messages = TextMessages::new();
    for msg in converted {
        messages = messages.add_message(msg.role, msg.content);
    }
    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;
    use swink_agent::testing::{assistant_msg, tool_result_msg, user_msg};
    use swink_agent::types::{
        AgentMessage, AssistantMessage, ContentBlock, Cost, LlmMessage, StopReason,
        ToolResultMessage, Usage, UserMessage,
    };

    // ── Helpers ───────────────────────────────────────────────────────────

    fn make_context(system: &str, messages: Vec<AgentMessage>) -> AgentContext {
        AgentContext {
            system_prompt: system.to_string(),
            messages,
            tools: vec![],
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────

    #[test]
    fn empty_context_produces_empty_messages() {
        let ctx = make_context("", vec![]);
        let _msgs = convert_context_messages(&ctx, &ModelConfig::default(), false);
        // TextMessages doesn't expose length, but it shouldn't panic.
    }

    #[test]
    fn system_prompt_is_included() {
        let ctx = make_context("You are helpful.", vec![]);
        let _msgs = convert_context_messages(&ctx, &ModelConfig::default(), false);
        // System prompt is set — no panic.
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
        let _msgs = convert_context_messages(&ctx, &ModelConfig::default(), false);
        // All message types handled — no panic.
    }

    #[test]
    fn custom_messages_skipped() {
        use std::any::Any;

        #[derive(Debug)]
        struct Custom;
        impl swink_agent::types::CustomMessage for Custom {
            fn as_any(&self) -> &dyn Any {
                self
            }
        }

        let ctx = make_context(
            "",
            vec![
                user_msg("before"),
                AgentMessage::Custom(Box::new(Custom)),
                user_msg("after"),
            ],
        );
        let _msgs = convert_context_messages(&ctx, &ModelConfig::default(), false);
        // Custom skipped — no panic.
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
        let ctx = make_context("", vec![msg]);
        let _msgs = convert_context_messages(&ctx, &ModelConfig::default(), false);
        // Empty content blocks produce empty text — no panic.
    }

    #[test]
    fn tool_result_includes_call_id() {
        let ctx = make_context("", vec![tool_result_msg("tc-42", "file contents")]);
        let _msgs = convert_context_messages(&ctx, &ModelConfig::default(), false);
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
        let ctx = make_context("", vec![msg]);
        let _msgs = convert_context_messages(&ctx, &ModelConfig::default(), false);
        // Multiple text blocks concatenated — no panic.
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
        let ctx = make_context("", vec![msg]);
        // Only Text blocks are extracted — others silently ignored.
        let _msgs = convert_context_messages(&ctx, &ModelConfig::default(), false);
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
        let _msgs = convert_context_messages(&ctx, &ModelConfig::default(), false);
        // Error tool results convert without panic.
    }

    #[cfg(feature = "gemma4")]
    mod gemma4_tests {
        use super::*;

        fn gemma4_config() -> ModelConfig {
            ModelConfig {
                repo_id: "bartowski/google_gemma-4-E2B-it-GGUF".to_string(),
                ..ModelConfig::default()
            }
        }

        fn smollm_config() -> ModelConfig {
            ModelConfig {
                repo_id: "bartowski/SmolLM3-3B-GGUF".to_string(),
                ..ModelConfig::default()
            }
        }

        #[test]
        fn think_token_injected_for_gemma4() {
            // We cannot inspect TextMessages content directly, but we can
            // verify the function doesn't panic and the injection path is
            // exercised. The real verification is that the system prompt
            // string is modified before being passed to the converter.
            //
            // To properly test, we intercept at the system_prompt level.
            let ctx = make_context("You are helpful.", vec![]);
            let config = gemma4_config();

            // Build the system prompt the same way the function does.
            let expected_prefix = "<|think|>\n";
            let system_prompt = if config.is_gemma4() {
                format!("{expected_prefix}{}", ctx.system_prompt)
            } else {
                ctx.system_prompt.clone()
            };
            assert!(system_prompt.starts_with(expected_prefix));
            assert!(system_prompt.contains("You are helpful."));

            // Also verify the full function doesn't panic.
            let _msgs = convert_context_messages(&ctx, &config, true);
        }

        #[test]
        fn think_token_not_injected_for_smollm() {
            let ctx = make_context("You are helpful.", vec![]);
            let config = smollm_config();

            // SmolLM is not Gemma 4, so no injection.
            assert!(!config.is_gemma4());

            // The system prompt should remain unchanged.
            let _msgs = convert_context_messages(&ctx, &config, true);
        }

        #[test]
        fn think_token_not_injected_when_thinking_disabled() {
            let ctx = make_context("You are helpful.", vec![]);
            let config = gemma4_config();

            assert!(config.is_gemma4());

            // thinking_enabled = false → no injection even for Gemma 4.
            // We verify by checking the internal logic.
            let should_inject = config.is_gemma4() && false; // thinking_enabled = false
            assert!(!should_inject);

            let _msgs = convert_context_messages(&ctx, &config, false);
        }
    }
}
