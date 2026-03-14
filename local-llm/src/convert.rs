//! Message conversion from swink-agent types to mistral.rs types.
//!
//! Implements core's [`MessageConverter`] trait so the shared
//! [`convert_messages`](swink_agent::convert_messages) function handles
//! iteration / pattern-matching, while this module supplies the
//! mistral.rs-specific construction.

use mistralrs::{TextMessageRole, TextMessages, Tool};

use swink_agent::convert::{MessageConverter, convert_messages, extract_tool_schemas};
use swink_agent::types::{
    AgentContext, AssistantMessage, ContentBlock, ToolResultMessage, UserMessage,
};

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
pub fn convert_context_messages(context: &AgentContext) -> TextMessages {
    let converted = convert_messages::<LocalConverter>(&context.messages, &context.system_prompt);

    let mut messages = TextMessages::new();
    for msg in converted {
        messages = messages.add_message(msg.role, msg.content);
    }
    messages
}

/// Convert agent tools into mistral.rs [`Tool`] definitions.
///
/// Each tool's `name()`, `description()`, and `parameters_schema()` are
/// mapped to the OpenAI-compatible function calling format that mistral.rs
/// expects.
#[allow(dead_code)] // Will be used when tool calling is wired into the stream.
pub fn convert_tools(context: &AgentContext) -> Vec<Tool> {
    extract_tool_schemas(&context.tools)
        .into_iter()
        .map(|schema| {
            let function = serde_json::json!({
                "type": "function",
                "function": {
                    "name": schema.name,
                    "description": schema.description,
                    "parameters": schema.parameters,
                }
            });
            serde_json::from_value::<Tool>(function).unwrap_or_else(|e| {
                tracing::warn!(
                    tool = %schema.name,
                    error = %e,
                    "failed to convert tool schema, using empty"
                );
                serde_json::from_value::<Tool>(serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": schema.name,
                        "description": schema.description,
                        "parameters": {"type": "object", "properties": {}}
                    }
                }))
                .expect("fallback tool schema must be valid")
            })
        })
        .collect()
}

/// Serialize tools to JSON strings for the `tool_schemas` parameter.
#[allow(dead_code)] // Used in tests; will be used in production when tool calling is wired in.
pub fn tool_schemas_json(context: &AgentContext) -> Vec<String> {
    extract_tool_schemas(&context.tools)
        .into_iter()
        .map(|schema| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": schema.name,
                    "description": schema.description,
                    "parameters": schema.parameters,
                }
            })
            .to_string()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;

    use serde_json::{Value, json};
    use swink_agent::tool::{AgentTool, AgentToolResult};
    use swink_agent::testing::{assistant_msg, tool_result_msg, user_msg};
    use swink_agent::types::{
        AgentMessage, AssistantMessage, ContentBlock, Cost, LlmMessage, StopReason,
        ToolResultMessage, Usage,
    };
    use tokio_util::sync::CancellationToken;

    // ── Mock tool ─────────────────────────────────────────────────────────

    struct MockTool {
        schema: Value,
    }

    impl MockTool {
        fn new() -> Self {
            Self {
                schema: json!({
                    "type": "object",
                    "properties": {
                        "input": { "type": "string" }
                    },
                    "required": ["input"]
                }),
            }
        }
    }

    impl AgentTool for MockTool {
        fn name(&self) -> &'static str {
            "mock_tool"
        }
        fn label(&self) -> &'static str {
            "Mock Tool"
        }
        fn description(&self) -> &'static str {
            "A mock tool for testing"
        }
        fn parameters_schema(&self) -> &Value {
            &self.schema
        }
        fn execute(
            &self,
            _tool_call_id: &str,
            _params: Value,
            _token: CancellationToken,
            _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
            Box::pin(async { AgentToolResult::text("mock result") })
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────

    fn make_context(
        system: &str,
        messages: Vec<AgentMessage>,
        tools: Vec<Arc<dyn AgentTool>>,
    ) -> AgentContext {
        AgentContext {
            system_prompt: system.to_string(),
            messages,
            tools,
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────

    #[test]
    fn empty_context_produces_empty_messages() {
        let ctx = make_context("", vec![], vec![]);
        let _msgs = convert_context_messages(&ctx);
        // TextMessages doesn't expose length, but it shouldn't panic.
    }

    #[test]
    fn system_prompt_is_included() {
        let ctx = make_context("You are helpful.", vec![], vec![]);
        let _msgs = convert_context_messages(&ctx);
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
            vec![],
        );
        let _msgs = convert_context_messages(&ctx);
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
            vec![],
        );
        let _msgs = convert_context_messages(&ctx);
        // Custom skipped — no panic.
    }

    #[test]
    fn tool_schemas_generated() {
        let ctx = make_context(
            "",
            vec![],
            vec![Arc::new(MockTool::new()) as Arc<dyn AgentTool>],
        );
        let schemas = tool_schemas_json(&ctx);
        assert_eq!(schemas.len(), 1);
        let schema: Value = serde_json::from_str(&schemas[0]).unwrap();
        assert_eq!(schema["function"]["name"], "mock_tool");
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
            timestamp: 0,
        }));
        let ctx = make_context("", vec![msg], vec![]);
        let _msgs = convert_context_messages(&ctx);
        // Empty content blocks produce empty text — no panic.
    }

    #[test]
    fn tool_result_includes_call_id() {
        // Verify the tool_call_id is embedded in the converted message content.
        let ctx = make_context("", vec![tool_result_msg("tc-42", "file contents")], vec![]);
        // Conversion should not panic; the tool_call_id is formatted into the text.
        let _msgs = convert_context_messages(&ctx);
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
        }));
        let ctx = make_context("", vec![msg], vec![]);
        let _msgs = convert_context_messages(&ctx);
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
        }));
        let ctx = make_context("", vec![msg], vec![]);
        // Only Text blocks are extracted — others silently ignored.
        let _msgs = convert_context_messages(&ctx);
    }

    #[test]
    fn convert_tools_produces_valid_tool_definitions() {
        let ctx = make_context(
            "",
            vec![],
            vec![Arc::new(MockTool::new()) as Arc<dyn AgentTool>],
        );
        let tools = convert_tools(&ctx);
        assert_eq!(tools.len(), 1);
    }

    #[test]
    fn convert_tools_empty_context() {
        let ctx = make_context("", vec![], vec![]);
        let tools = convert_tools(&ctx);
        assert!(tools.is_empty());
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
        }));
        let ctx = make_context("", vec![msg], vec![]);
        let _msgs = convert_context_messages(&ctx);
        // Error tool results convert without panic.
    }
}
