//! Tests for `convert`.
#![cfg(test)]

use super::*;

use serde_json::json;
use swink_agent::testing::{assistant_msg, tool_result_msg, user_msg};
use swink_agent::{
    AgentMessage, AssistantMessage, ContentBlock, LlmMessage, StopReason, ToolResultMessage,
    UserMessage,
};

fn make_context(system: &str, messages: Vec<AgentMessage>) -> AgentContext {
    AgentContext::new(system.to_string(), messages, vec![])
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
    let msg = AgentMessage::Llm(LlmMessage::Assistant(
        AssistantMessage::new(vec![], String::new(), String::new()).with_timestamp(0),
    ));
    let ctx = make_context("sys", vec![msg]);
    let msgs = convert_context_messages(&ctx, &smollm_config(), false);
    assert_eq!(msgs[1].role, "assistant");
}

#[test]
fn assistant_tool_calls_preserved() {
    let msg = AgentMessage::Llm(LlmMessage::Assistant(
        AssistantMessage::new(
            vec![
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
            String::new(),
            String::new(),
        )
        .with_stop_reason(StopReason::ToolUse)
        .with_timestamp(0),
    ));
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
    let msg = AgentMessage::Llm(LlmMessage::User(
        UserMessage::new(vec![
            ContentBlock::Text {
                text: "Hello ".to_string(),
            },
            ContentBlock::Text {
                text: "world!".to_string(),
            },
        ])
        .with_timestamp(0),
    ));
    let ctx = make_context("sys", vec![msg]);
    let msgs = convert_context_messages(&ctx, &smollm_config(), false);
    assert_eq!(msgs[1].content, "Hello world!");
}

#[test]
fn non_text_content_blocks_ignored() {
    let msg = AgentMessage::Llm(LlmMessage::User(
        UserMessage::new(vec![
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
        ])
        .with_timestamp(0),
    ));
    let ctx = make_context("sys", vec![msg]);
    let msgs = convert_context_messages(&ctx, &smollm_config(), false);
    assert_eq!(msgs[1].content, "text part");
}

#[test]
fn tool_result_error_message_no_panic() {
    let msg = AgentMessage::Llm(LlmMessage::ToolResult(
        ToolResultMessage::new(
            "tc-err",
            vec![ContentBlock::Text {
                text: "error: command failed".to_string(),
            }],
        )
        .with_is_error(true)
        .with_timestamp(0),
    ));
    let ctx = make_context("", vec![msg]);
    let _msgs = convert_context_messages(&ctx, &smollm_config(), false);
}

#[cfg(feature = "gemma4")]
#[test]
fn tool_result_formatting() {
    use swink_agent::ToolResultMessage;

    let result = ToolResultMessage::new(
        "read_file",
        vec![ContentBlock::Text {
            text: "file contents".to_string(),
        }],
    )
    .with_timestamp(0);

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
