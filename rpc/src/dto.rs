//! Wire DTOs for JSON-RPC message parameters.
//!
//! These types sit between the JSON wire format and the core `swink-agent` types.
//! They are kept in this crate to avoid adding serde derives to core types
//! that were not designed with a wire protocol in mind.

use serde::{Deserialize, Serialize};
use swink_agent::{ToolApproval, ToolApprovalRequest};

// ─── Handshake ────────────────────────────────────────────────────────────────

/// Payload for the `initialize` notification sent by the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeParams {
    pub protocol_version: String,
    #[serde(default)]
    pub client: ClientInfo,
}

/// Payload for the `initialized` notification sent by the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializedParams {
    pub protocol_version: String,
    #[serde(default)]
    pub server: ServerInfo,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

// ─── prompt ───────────────────────────────────────────────────────────────────

/// Parameters for the `prompt` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptParams {
    /// The user's text message.
    pub text: String,
    /// Optionally continue a previous session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// Response to the `prompt` request — sent once the agent accepts the input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptResult {
    pub turn_id: String,
}

// ─── tool.approve ─────────────────────────────────────────────────────────────

/// Parameters for the `tool.approve` request sent by the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolApprovalRequestDto {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    pub requires_approval: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<serde_json::Value>,
}

impl From<&ToolApprovalRequest> for ToolApprovalRequestDto {
    fn from(req: &ToolApprovalRequest) -> Self {
        Self {
            id: req.tool_call_id.clone(),
            name: req.tool_name.clone(),
            arguments: req.arguments.clone(),
            requires_approval: req.requires_approval,
            context: req.context.clone(),
        }
    }
}

/// Response to the `tool.approve` request sent by the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "decision")]
pub enum ToolApprovalDto {
    Approved,
    Rejected,
    ApprovedWith { value: serde_json::Value },
}

impl From<ToolApprovalDto> for ToolApproval {
    fn from(dto: ToolApprovalDto) -> Self {
        match dto {
            ToolApprovalDto::Approved => Self::Approved,
            ToolApprovalDto::Rejected => Self::Rejected,
            ToolApprovalDto::ApprovedWith { value } => Self::ApprovedWith(value),
        }
    }
}

impl From<&ToolApproval> for ToolApprovalDto {
    fn from(approval: &ToolApproval) -> Self {
        match approval {
            ToolApproval::Approved => Self::Approved,
            ToolApproval::Rejected => Self::Rejected,
            ToolApproval::ApprovedWith(v) => Self::ApprovedWith { value: v.clone() },
        }
    }
}

// ─── Protocol constants ───────────────────────────────────────────────────────

pub const PROTOCOL_VERSION: &str = "1.0";

pub mod method {
    pub const INITIALIZE: &str = "initialize";
    pub const INITIALIZED: &str = "initialized";
    pub const PROMPT: &str = "prompt";
    pub const CANCEL: &str = "cancel";
    pub const SHUTDOWN: &str = "shutdown";
    pub const AGENT_EVENT: &str = "agent.event";
    pub const TOOL_APPROVE: &str = "tool.approve";
}
