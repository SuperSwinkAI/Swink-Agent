//! Wire DTOs for JSON-RPC message parameters.
//!
//! These types sit between the JSON wire format and the core `swink-agent` types.
//! They are kept in this crate to avoid adding serde derives to core types
//! that were not designed with a wire protocol in mind.

use serde::{Deserialize, Serialize};
use swink_agent::{ToolApproval, ToolApprovalRequest};

#[cfg(any(unix, test))]
use crate::jsonrpc::RpcError;

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

#[cfg(any(unix, test))]
pub(crate) fn parse_initialize_params(
    params: Option<serde_json::Value>,
) -> Result<InitializeParams, RpcError> {
    let params: InitializeParams = parse_handshake_params(params, "initialize")?;
    ensure_protocol_version(&params.protocol_version)?;
    Ok(params)
}

#[cfg(any(unix, test))]
pub(crate) fn parse_initialized_params(
    params: Option<serde_json::Value>,
) -> Result<InitializedParams, RpcError> {
    let params: InitializedParams = parse_handshake_params(params, "initialized")?;
    ensure_protocol_version(&params.protocol_version)?;
    Ok(params)
}

#[cfg(any(unix, test))]
fn parse_handshake_params<T>(params: Option<serde_json::Value>, method: &str) -> Result<T, RpcError>
where
    T: for<'de> Deserialize<'de>,
{
    let Some(params) = params else {
        return Err(RpcError::invalid_request(format!(
            "missing {method} params"
        )));
    };
    serde_json::from_value(params)
        .map_err(|e| RpcError::invalid_request(format!("invalid {method} params: {e}")))
}

#[cfg(any(unix, test))]
fn ensure_protocol_version(actual: &str) -> Result<(), RpcError> {
    if actual == PROTOCOL_VERSION {
        return Ok(());
    }

    Err(RpcError::protocol_mismatch(format!(
        "protocol version mismatch: expected {PROTOCOL_VERSION}, got {actual}"
    )))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_params_accept_current_protocol_version() {
        let params = serde_json::json!({
            "protocol_version": PROTOCOL_VERSION,
            "client": {
                "name": "test-client",
                "version": "0.1.0"
            }
        });

        let parsed = parse_initialize_params(Some(params)).unwrap();

        assert_eq!(parsed.protocol_version, PROTOCOL_VERSION);
        assert_eq!(parsed.client.name, "test-client");
    }

    #[test]
    fn initialize_params_reject_protocol_version_mismatch() {
        let params = serde_json::json!({
            "protocol_version": "0.9",
            "client": {
                "name": "old-client",
                "version": "0.1.0"
            }
        });

        let err = parse_initialize_params(Some(params)).unwrap_err();

        assert_eq!(err.code, RpcError::PROTOCOL_MISMATCH);
    }

    #[test]
    fn initialized_params_reject_protocol_version_mismatch() {
        let params = serde_json::json!({
            "protocol_version": "2.0",
            "server": {
                "name": "future-server",
                "version": "0.1.0"
            }
        });

        let err = parse_initialized_params(Some(params)).unwrap_err();

        assert_eq!(err.code, RpcError::PROTOCOL_MISMATCH);
    }

    #[test]
    fn handshake_params_reject_missing_protocol_version() {
        let params = serde_json::json!({
            "client": {
                "name": "broken-client",
                "version": "0.1.0"
            }
        });

        let err = parse_initialize_params(Some(params)).unwrap_err();

        assert_eq!(err.code, RpcError::INVALID_REQUEST);
    }

    #[test]
    fn prompt_params_omit_absent_session_id_and_round_trip_present_session_id() {
        let params = PromptParams {
            text: "hello rpc".into(),
            session_id: None,
        };

        let encoded = serde_json::to_value(&params).unwrap();

        assert_eq!(encoded["text"], "hello rpc");
        assert!(
            encoded.get("session_id").is_none(),
            "empty session ids should stay off the wire"
        );

        let decoded: PromptParams = serde_json::from_value(serde_json::json!({
            "text": "continue",
            "session_id": "session-1"
        }))
        .unwrap();

        assert_eq!(decoded.text, "continue");
        assert_eq!(decoded.session_id.as_deref(), Some("session-1"));
    }

    #[test]
    fn tool_approval_request_dto_preserves_core_request_payload() {
        let request = ToolApprovalRequest {
            tool_call_id: "call-1".into(),
            tool_name: "write_file".into(),
            arguments: serde_json::json!({"path": "notes.md", "content": "ok"}),
            requires_approval: true,
            context: Some(serde_json::json!({"cwd": "/workspace"})),
        };

        let dto = ToolApprovalRequestDto::from(&request);
        let encoded = serde_json::to_value(&dto).unwrap();

        assert_eq!(encoded["id"], "call-1");
        assert_eq!(encoded["name"], "write_file");
        assert_eq!(encoded["arguments"]["path"], "notes.md");
        assert_eq!(encoded["requires_approval"], true);
        assert_eq!(encoded["context"]["cwd"], "/workspace");
    }

    #[test]
    fn tool_approval_dto_round_trips_all_decisions() {
        let approved =
            serde_json::to_value(ToolApprovalDto::from(&ToolApproval::Approved)).unwrap();
        let rejected =
            serde_json::to_value(ToolApprovalDto::from(&ToolApproval::Rejected)).unwrap();
        let modified_value = serde_json::json!({"path": "safe.md"});
        let modified = serde_json::to_value(ToolApprovalDto::from(&ToolApproval::ApprovedWith(
            modified_value.clone(),
        )))
        .unwrap();

        assert_eq!(approved, serde_json::json!({"decision": "approved"}));
        assert_eq!(rejected, serde_json::json!({"decision": "rejected"}));
        assert_eq!(
            modified,
            serde_json::json!({"decision": "approved_with", "value": modified_value})
        );

        assert!(matches!(
            ToolApproval::from(serde_json::from_value::<ToolApprovalDto>(approved).unwrap()),
            ToolApproval::Approved
        ));
        assert!(matches!(
            ToolApproval::from(serde_json::from_value::<ToolApprovalDto>(rejected).unwrap()),
            ToolApproval::Rejected
        ));
        assert!(matches!(
            ToolApproval::from(serde_json::from_value::<ToolApprovalDto>(modified).unwrap()),
            ToolApproval::ApprovedWith(value) if value == modified_value
        ));
    }
}
