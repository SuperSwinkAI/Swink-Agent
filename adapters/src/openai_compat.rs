//! Shared OpenAI-compatible request/response types.
//!
//! Azure, Mistral, xAI, and plain `OpenAI` all use structurally identical
//! message, tool, and streaming chunk types. This module defines them once
//! so every adapter can reuse them without copy-paste.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use swink_agent::AgentTool;
use swink_agent::ContentBlock;
use swink_agent::types::{
    AssistantMessage as HarnessAssistantMessage, ToolResultMessage, UserMessage,
};

use crate::convert::{MessageConverter, extract_tool_schemas};

// ─── Request types ──────────────────────────────────────────────────────────

/// Message in `OpenAI`'s chat completions format.
#[derive(Debug, Serialize)]
pub struct OaiMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OaiToolCallRequest>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// Tool call in the request (assistant message replay).
#[derive(Debug, Serialize)]
pub struct OaiToolCallRequest {
    pub id: String,
    pub r#type: String,
    pub function: OaiFunctionCallRequest,
}

/// Function call details in a request tool call.
#[derive(Debug, Serialize)]
pub struct OaiFunctionCallRequest {
    pub name: String,
    pub arguments: String,
}

/// Tool definition in `OpenAI`'s format.
#[derive(Debug, Serialize)]
pub struct OaiTool {
    pub r#type: String,
    pub function: OaiToolDef,
}

/// Tool function definition.
#[derive(Debug, Serialize)]
pub struct OaiToolDef {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// Stream options for the request.
#[derive(Debug, Serialize)]
pub struct OaiStreamOptions {
    pub include_usage: bool,
}

/// Full request body for OpenAI-compatible `/v1/chat/completions`.
#[derive(Debug, Serialize)]
pub struct OaiChatRequest {
    pub model: String,
    pub messages: Vec<OaiMessage>,
    pub stream: bool,
    pub stream_options: OaiStreamOptions,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<OaiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,
}

// ─── Response / streaming types ─────────────────────────────────────────────

/// A single SSE chunk from an OpenAI-compatible streaming response.
#[derive(Deserialize)]
pub struct OaiChunk {
    #[serde(default)]
    pub choices: Vec<OaiChoice>,
    #[serde(default)]
    pub usage: Option<OaiUsage>,
}

/// A choice in a streaming chunk.
#[derive(Deserialize)]
pub struct OaiChoice {
    #[serde(default)]
    pub delta: OaiDelta,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

/// The delta portion of a streaming choice.
#[derive(Default, Deserialize)]
pub struct OaiDelta {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<OaiToolCallDelta>>,
}

/// A tool call delta in a streaming response.
#[derive(Deserialize)]
pub struct OaiToolCallDelta {
    pub index: usize,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub function: Option<OaiFunctionDelta>,
}

/// Function delta in a tool call.
#[derive(Deserialize)]
pub struct OaiFunctionDelta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

/// Usage information in the response.
#[derive(Deserialize)]
pub struct OaiUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
}

// ─── Tool call state tracking ───────────────────────────────────────────────

/// Tracks the accumulated state of a single tool call during streaming.
pub struct ToolCallState {
    pub arguments: String,
    pub started: bool,
    pub content_index: usize,
}

// ─── MessageConverter impl ──────────────────────────────────────────────────

/// Marker type for OpenAI-compatible message conversion.
///
/// Reused by any adapter whose wire format matches the `OpenAI` chat completions
/// message schema (`OpenAI`, Azure, Mistral, xAI, etc.).
pub struct OaiConverter;

impl MessageConverter for OaiConverter {
    type Message = OaiMessage;

    fn system_message(system_prompt: &str) -> Option<OaiMessage> {
        Some(OaiMessage {
            role: "system".to_string(),
            content: Some(system_prompt.to_string()),
            tool_calls: None,
            tool_call_id: None,
        })
    }

    fn user_message(user: &UserMessage) -> OaiMessage {
        let content = ContentBlock::extract_text(&user.content);
        OaiMessage {
            role: "user".to_string(),
            content: Some(content),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    fn assistant_message(assistant: &HarnessAssistantMessage) -> OaiMessage {
        let mut content = String::new();
        let mut tool_calls = Vec::new();
        for block in &assistant.content {
            match block {
                ContentBlock::Text { text } => {
                    content.push_str(text);
                }
                ContentBlock::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                } => {
                    tool_calls.push(OaiToolCallRequest {
                        id: id.clone(),
                        r#type: "function".to_string(),
                        function: OaiFunctionCallRequest {
                            name: name.clone(),
                            arguments: arguments.to_string(),
                        },
                    });
                }
                _ => {}
            }
        }
        OaiMessage {
            role: "assistant".to_string(),
            content: if content.is_empty() {
                None
            } else {
                Some(content)
            },
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
            tool_call_id: None,
        }
    }

    fn tool_result_message(result: &ToolResultMessage) -> OaiMessage {
        let content = ContentBlock::extract_text(&result.content);
        OaiMessage {
            role: "tool".to_string(),
            content: Some(content),
            tool_calls: None,
            tool_call_id: Some(result.tool_call_id.clone()),
        }
    }
}

// ─── Shared helpers ─────────────────────────────────────────────────────────

/// Build the `tools` vec and `tool_choice` from the agent context's tool list.
pub fn build_oai_tools(tools: &[Arc<dyn AgentTool>]) -> (Vec<OaiTool>, Option<String>) {
    let oai_tools: Vec<OaiTool> = extract_tool_schemas(tools)
        .into_iter()
        .map(|s| OaiTool {
            r#type: "function".to_string(),
            function: OaiToolDef {
                name: s.name,
                description: s.description,
                parameters: s.parameters,
            },
        })
        .collect();
    let tool_choice = if oai_tools.is_empty() {
        None
    } else {
        Some("auto".to_string())
    };
    (oai_tools, tool_choice)
}
