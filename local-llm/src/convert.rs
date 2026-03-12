//! Message conversion from swink-agent types to mistral.rs types.
//!
//! Standalone conversion (not using adapters' `MessageConverter` — different
//! crate). Converts [`AgentContext`] messages and tools into mistral.rs
//! [`TextMessages`] and [`Tool`] definitions.

use mistralrs::{TextMessageRole, TextMessages, Tool};

use swink_agent::types::{
    AgentContext, AgentMessage, AssistantMessage, ContentBlock, LlmMessage, ToolResultMessage,
    UserMessage,
};

// ─── Message Conversion ─────────────────────────────────────────────────────

/// Convert an [`AgentContext`] into mistral.rs [`TextMessages`].
///
/// Mapping:
/// - `system_prompt` → `System` role
/// - [`UserMessage`] → `User` role
/// - [`AssistantMessage`] → `Assistant` role
/// - [`ToolResultMessage`] → `Tool` role (with `tool_call_id`)
/// - `CustomMessage` → skipped
pub fn convert_messages(context: &AgentContext) -> TextMessages {
    let mut messages = TextMessages::new();

    if !context.system_prompt.is_empty() {
        messages = messages.add_message(TextMessageRole::System, &context.system_prompt);
    }

    for msg in &context.messages {
        let AgentMessage::Llm(llm) = msg else {
            continue;
        };
        match llm {
            LlmMessage::User(user) => {
                messages = convert_user(messages, user);
            }
            LlmMessage::Assistant(assistant) => {
                messages = convert_assistant(messages, assistant);
            }
            LlmMessage::ToolResult(result) => {
                messages = convert_tool_result(messages, result);
            }
        }
    }

    messages
}

fn convert_user(messages: TextMessages, user: &UserMessage) -> TextMessages {
    let text = ContentBlock::extract_text(&user.content);
    messages.add_message(TextMessageRole::User, text)
}

fn convert_assistant(messages: TextMessages, assistant: &AssistantMessage) -> TextMessages {
    let text = ContentBlock::extract_text(&assistant.content);
    messages.add_message(TextMessageRole::Assistant, text)
}

fn convert_tool_result(messages: TextMessages, result: &ToolResultMessage) -> TextMessages {
    let text = ContentBlock::extract_text(&result.content);
    // Tool results include the tool_call_id for matching.
    let content = format!("[tool_call_id: {}]\n{text}", result.tool_call_id);
    messages.add_message(TextMessageRole::Tool, content)
}

// ─── Tool Conversion ────────────────────────────────────────────────────────

/// Convert agent tools into mistral.rs [`Tool`] definitions.
///
/// Each tool's `name()`, `description()`, and `parameters_schema()` are
/// mapped to the OpenAI-compatible function calling format that mistral.rs
/// expects.
#[allow(dead_code)] // Will be used when tool calling is wired into the stream.
pub fn convert_tools(context: &AgentContext) -> Vec<Tool> {
    context
        .tools
        .iter()
        .map(|t| {
            let function = serde_json::json!({
                "type": "function",
                "function": {
                    "name": t.name(),
                    "description": t.description(),
                    "parameters": t.parameters_schema(),
                }
            });
            serde_json::from_value::<Tool>(function).unwrap_or_else(|e| {
                tracing::warn!(
                    tool = t.name(),
                    error = %e,
                    "failed to convert tool schema, using empty"
                );
                serde_json::from_value::<Tool>(serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name(),
                        "description": t.description(),
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
    context
        .tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": t.name(),
                    "description": t.description(),
                    "parameters": t.parameters_schema(),
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
    use swink_agent::types::{
        AgentMessage, AssistantMessage, ContentBlock, Cost, LlmMessage, StopReason,
        ToolResultMessage, Usage, UserMessage,
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
        fn name(&self) -> &str {
            "mock_tool"
        }
        fn label(&self) -> &str {
            "Mock Tool"
        }
        fn description(&self) -> &str {
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

    fn user_msg(text: &str) -> AgentMessage {
        AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            timestamp: 0,
        }))
    }

    fn assistant_msg(text: &str) -> AgentMessage {
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

    fn tool_result_msg(id: &str, text: &str) -> AgentMessage {
        AgentMessage::Llm(LlmMessage::ToolResult(ToolResultMessage {
            tool_call_id: id.to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            is_error: false,
            timestamp: 0,
            details: serde_json::Value::Null,
        }))
    }

    // ── Tests ─────────────────────────────────────────────────────────────

    #[test]
    fn empty_context_produces_empty_messages() {
        let ctx = make_context("", vec![], vec![]);
        let _msgs = convert_messages(&ctx);
        // TextMessages doesn't expose length, but it shouldn't panic.
    }

    #[test]
    fn system_prompt_is_included() {
        let ctx = make_context("You are helpful.", vec![], vec![]);
        let _msgs = convert_messages(&ctx);
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
        let _msgs = convert_messages(&ctx);
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
        let _msgs = convert_messages(&ctx);
        // Custom skipped — no panic.
    }

    #[test]
    fn tool_schemas_generated() {
        let ctx = make_context("", vec![], vec![Arc::new(MockTool::new()) as Arc<dyn AgentTool>]);
        let schemas = tool_schemas_json(&ctx);
        assert_eq!(schemas.len(), 1);
        let schema: Value = serde_json::from_str(&schemas[0]).unwrap();
        assert_eq!(schema["function"]["name"], "mock_tool");
    }
}
