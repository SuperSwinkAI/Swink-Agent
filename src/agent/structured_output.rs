use serde_json::Value;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::error::AgentError;
use crate::tool::{AgentToolResult, ToolFuture};
use crate::types::{AgentMessage, ContentBlock, LlmMessage};
use crate::util::now_timestamp;

use super::Agent;

impl Agent {
    /// Run a structured output extraction loop.
    pub async fn structured_output(
        &mut self,
        prompt: String,
        schema: Value,
    ) -> Result<Value, AgentError> {
        let tool = Arc::new(StructuredOutputTool {
            schema: schema.clone(),
        });

        self.state.tools.push(tool);
        let result = self
            .run_structured_output_attempts(prompt, schema.clone())
            .await;
        self.remove_structured_output_tool();
        result
    }

    /// Run a structured output extraction loop, blocking the current thread.
    pub fn structured_output_sync(
        &mut self,
        prompt: String,
        schema: Value,
    ) -> Result<Value, AgentError> {
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(self.structured_output(prompt, schema))
    }

    /// Run structured output extraction and deserialize into a typed result.
    pub async fn structured_output_typed<T: serde::de::DeserializeOwned>(
        &mut self,
        prompt: String,
        schema: Value,
    ) -> Result<T, AgentError> {
        let value = self.structured_output(prompt, schema).await?;
        serde_json::from_value(value).map_err(|e| AgentError::StructuredOutputFailed {
            attempts: 1,
            last_error: format!("deserialization failed: {e}"),
        })
    }

    /// Run structured output extraction, deserialize into a typed result, blocking.
    pub fn structured_output_typed_sync<T: serde::de::DeserializeOwned>(
        &mut self,
        prompt: String,
        schema: Value,
    ) -> Result<T, AgentError> {
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(self.structured_output_typed(prompt, schema))
    }

    async fn run_structured_output_attempts(
        &mut self,
        prompt: String,
        schema: Value,
    ) -> Result<Value, AgentError> {
        let mut last_error = String::new();
        let max_retries = self.structured_output_max_retries;

        for attempt in 0..=max_retries {
            let result = if attempt == 0 {
                let user_msg = AgentMessage::Llm(LlmMessage::User(crate::types::UserMessage {
                    content: vec![ContentBlock::Text {
                        text: prompt.clone(),
                    }],
                    timestamp: now_timestamp(),
                    cache_hint: None,
                }));
                self.prompt_async(vec![user_msg]).await?
            } else {
                self.continue_async().await?
            };

            match extract_structured_output(&result, &schema) {
                Ok(value) => return Ok(value),
                Err(e) => {
                    last_error.clone_from(&e);
                    if attempt < max_retries {
                        let feedback = AgentMessage::Llm(LlmMessage::ToolResult(
                            crate::types::ToolResultMessage {
                                tool_call_id: find_structured_output_call_id(&result)
                                    .unwrap_or_default(),
                                content: vec![ContentBlock::Text {
                                    text: format!(
                                        "Validation failed: {e}. Please try again with valid \
                                         output."
                                    ),
                                }],
                                is_error: true,
                                timestamp: now_timestamp(),
                                details: serde_json::Value::Null,
                                cache_hint: None,
                            },
                        ));
                        self.state.messages.push(feedback);
                    }
                }
            }
        }

        Err(AgentError::StructuredOutputFailed {
            attempts: max_retries + 1,
            last_error,
        })
    }

    fn remove_structured_output_tool(&mut self) {
        self.state
            .tools
            .retain(|t| t.name() != "__structured_output");
    }
}

/// Synthetic tool used for structured output extraction.
struct StructuredOutputTool {
    schema: Value,
}

#[allow(clippy::unnecessary_literal_bound)]
impl crate::tool::AgentTool for StructuredOutputTool {
    fn name(&self) -> &str {
        "__structured_output"
    }

    fn label(&self) -> &str {
        "Structured Output"
    }

    fn description(&self) -> &str {
        "Return structured data matching the required JSON schema. Call this tool with the \
         requested data as the arguments."
    }

    fn parameters_schema(&self) -> &Value {
        &self.schema
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: std::sync::Arc<std::sync::RwLock<crate::SessionState>>,
        _credential: Option<crate::credential::ResolvedCredential>,
    ) -> ToolFuture<'_> {
        Box::pin(async move {
            AgentToolResult::text(serde_json::to_string(&params).unwrap_or_default())
        })
    }
}

fn extract_structured_output(result: &crate::types::AgentResult, schema: &Value) -> Result<Value, String> {
    for msg in &result.messages {
        if let AgentMessage::Llm(LlmMessage::Assistant(assistant)) = msg {
            for block in &assistant.content {
                if let ContentBlock::ToolCall {
                    name, arguments, ..
                } = block
                    && name == "__structured_output"
                {
                    let validation = crate::tool::validate_tool_arguments(schema, arguments);
                    match validation {
                        Ok(()) => return Ok(arguments.clone()),
                        Err(errors) => return Err(errors.join("; ")),
                    }
                }
            }
        }
    }
    Err("no __structured_output tool call found in response".to_string())
}

fn find_structured_output_call_id(result: &crate::types::AgentResult) -> Option<String> {
    for msg in &result.messages {
        if let AgentMessage::Llm(LlmMessage::Assistant(assistant)) = msg {
            for block in &assistant.content {
                if let ContentBlock::ToolCall { name, id, .. } = block
                    && name == "__structured_output"
                {
                    return Some(id.clone());
                }
            }
        }
    }
    None
}
