//! Wire DTOs for JSON-RPC message parameters.
//!
//! These types sit between the JSON wire format and the core `swink-agent` types.
//! They are kept in this crate to avoid adding serde derives to core types
//! that were not designed with a wire protocol in mind.

use serde::{Deserialize, Serialize};
use swink_agent::{ApprovalMode, ModelSpec, ThinkingLevel, ToolApproval, ToolApprovalRequest};

// Used only by the handshake parsers below (and their tests), which are gated
// on the `client`/`server` features — so this import carries the same gate.
#[cfg(any(all(unix, any(feature = "client", feature = "server")), test))]
use crate::jsonrpc::RpcError;

// ─── Handshake ────────────────────────────────────────────────────────────────

/// Payload for the `initialize` notification sent by the client.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeParams {
    pub protocol_version: String,
    #[serde(default)]
    pub client: ClientInfo,
}

impl InitializeParams {
    /// Construct with the given protocol version and default `client` info.
    #[must_use]
    pub fn new(protocol_version: impl Into<String>) -> Self {
        Self {
            protocol_version: protocol_version.into(),
            client: ClientInfo::default(),
        }
    }

    /// Set the client info.
    #[must_use]
    pub fn with_client(mut self, client: ClientInfo) -> Self {
        self.client = client;
        self
    }
}

/// Payload for the `initialized` notification sent by the server.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializedParams {
    pub protocol_version: String,
    #[serde(default)]
    pub server: ServerInfo,
}

impl InitializedParams {
    /// Construct with the given protocol version and default `server` info.
    #[must_use]
    pub fn new(protocol_version: impl Into<String>) -> Self {
        Self {
            protocol_version: protocol_version.into(),
            server: ServerInfo::default(),
        }
    }

    /// Set the server info.
    #[must_use]
    pub fn with_server(mut self, server: ServerInfo) -> Self {
        self.server = server;
        self
    }
}

#[non_exhaustive]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
}

impl ClientInfo {
    /// Construct a new `ClientInfo`.
    #[must_use]
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
        }
    }
}

#[non_exhaustive]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

impl ServerInfo {
    /// Construct a new `ServerInfo`.
    #[must_use]
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
        }
    }
}

// Called only from the unix `run_session` path in `server.rs`, which is itself
// behind the `server` feature — plus this module's own tests.
#[cfg(any(all(unix, feature = "server"), test))]
pub(crate) fn parse_initialize_params(
    params: Option<serde_json::Value>,
) -> Result<InitializeParams, RpcError> {
    let params: InitializeParams = parse_handshake_params(params, "initialize")?;
    ensure_protocol_version(&params.protocol_version)?;
    Ok(params)
}

// Called only from the unix handshake path in `client.rs`, which is itself
// behind the `client` feature — plus this module's own tests.
#[cfg(any(all(unix, feature = "client"), test))]
pub(crate) fn parse_initialized_params(
    params: Option<serde_json::Value>,
) -> Result<InitializedParams, RpcError> {
    let params: InitializedParams = parse_handshake_params(params, "initialized")?;
    ensure_protocol_version(&params.protocol_version)?;
    Ok(params)
}

// Shared by both handshake parsers above; live whenever either of them is.
#[cfg(any(all(unix, any(feature = "client", feature = "server")), test))]
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

// Shared by both handshake parsers above; live whenever either of them is.
#[cfg(any(all(unix, any(feature = "client", feature = "server")), test))]
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
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptParams {
    /// The user's text message.
    pub text: String,
    /// Optionally continue a previous session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl PromptParams {
    /// Construct a new `PromptParams` with no session id.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            session_id: None,
        }
    }

    /// Continue an existing session.
    #[must_use]
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }
}

/// Response to the `prompt` request — sent once the agent accepts the input.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptResult {
    pub turn_id: String,
}

impl PromptResult {
    /// Construct a new `PromptResult`.
    #[must_use]
    pub fn new(turn_id: impl Into<String>) -> Self {
        Self {
            turn_id: turn_id.into(),
        }
    }
}

// ─── tool.approve ─────────────────────────────────────────────────────────────

/// Parameters for the `tool.approve` request sent by the server.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolApprovalRequestDto {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    pub requires_approval: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<serde_json::Value>,
}

impl ToolApprovalRequestDto {
    /// Construct a new `ToolApprovalRequestDto` with no context.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
        requires_approval: bool,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
            requires_approval,
            context: None,
        }
    }

    /// Attach context data.
    #[must_use]
    pub fn with_context(mut self, context: serde_json::Value) -> Self {
        self.context = Some(context);
        self
    }
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
#[non_exhaustive]
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
            ToolApproval::ApprovedWith(v) => Self::ApprovedWith { value: v.clone() },
            // Covers ToolApproval::Rejected and, since ToolApproval is
            // #[non_exhaustive], any unknown future variant — fail closed
            // rather than silently approving a tool call.
            _ => Self::Rejected,
        }
    }
}

// ─── Control plane (protocol 1.1) ─────────────────────────────────────────────

/// Empty acknowledgement result for control-plane requests.
///
/// Returned by the methods that carry no response data (`model.set`,
/// `thinking.set`, `approval.set`, `system_prompt.set`, `agent.reset`,
/// `plan.enter`, `plan.exit`, `session.restore`). Serializes as `{}` on
/// the wire.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Ack {}

impl Ack {
    /// Construct a new `Ack`.
    #[must_use]
    pub const fn new() -> Self {
        Self {}
    }
}

/// Result of the `model.list` request.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelListResult {
    /// Models registered on the server via
    /// `AgentOptions::with_available_models` (may be empty).
    pub available: Vec<ModelSpec>,
    /// The model the agent is currently using.
    pub current: ModelSpec,
}

impl ModelListResult {
    /// Construct a new `ModelListResult`.
    #[must_use]
    pub fn new(available: Vec<ModelSpec>, current: ModelSpec) -> Self {
        Self { available, current }
    }
}

/// Parameters for the `model.set` request.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSetParams {
    /// The model to switch to.
    pub model: ModelSpec,
}

impl ModelSetParams {
    /// Construct a new `ModelSetParams`.
    #[must_use]
    pub fn new(model: ModelSpec) -> Self {
        Self { model }
    }
}

/// Parameters for the `thinking.set` request.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingSetParams {
    /// The thinking level to apply to the current model.
    pub level: ThinkingLevel,
}

impl ThinkingSetParams {
    /// Construct a new `ThinkingSetParams`.
    #[must_use]
    pub fn new(level: ThinkingLevel) -> Self {
        Self { level }
    }
}

/// Result of the `approval.get` request.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalGetResult {
    /// The agent's current approval mode.
    pub mode: ApprovalMode,
}

impl ApprovalGetResult {
    /// Construct a new `ApprovalGetResult`.
    #[must_use]
    pub fn new(mode: ApprovalMode) -> Self {
        Self { mode }
    }
}

/// Parameters for the `approval.set` request.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalSetParams {
    /// The approval mode to switch to.
    pub mode: ApprovalMode,
}

impl ApprovalSetParams {
    /// Construct a new `ApprovalSetParams`.
    #[must_use]
    pub fn new(mode: ApprovalMode) -> Self {
        Self { mode }
    }
}

/// Parameters for the `system_prompt.set` request.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemPromptSetParams {
    /// The replacement system prompt.
    pub prompt: String,
}

impl SystemPromptSetParams {
    /// Construct a new `SystemPromptSetParams`.
    #[must_use]
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
        }
    }
}

/// A portable session snapshot — the result of `session.snapshot` and the
/// parameters of `session.restore`.
///
/// `messages` uses the same per-message representation `swink-agent-memory`
/// writes to JSONL: LLM messages are raw `LlmMessage` JSON, custom messages
/// are a `{"type": ..., "data": ..., "_custom": true}` envelope. A client can
/// therefore feed each element to a `SessionStore` line-for-line (and vice
/// versa). `state` is the serialized `swink_agent::SessionState` snapshot.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// The message transcript, one JSON value per message.
    pub messages: Vec<serde_json::Value>,
    /// The session key-value state, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<serde_json::Value>,
}

impl SessionSnapshot {
    /// Construct a new `SessionSnapshot`.
    #[must_use]
    pub fn new(messages: Vec<serde_json::Value>, state: Option<serde_json::Value>) -> Self {
        Self { messages, state }
    }
}

/// Result of the `context.compact` request.
#[non_exhaustive]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompactResult {
    /// The last transformer's report, or `None` when no transformer is
    /// configured or every transformer declined (history under budget).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<swink_agent::CompactionReport>,
}

impl CompactResult {
    /// Construct a new `CompactResult`.
    #[must_use]
    pub const fn new(report: Option<swink_agent::CompactionReport>) -> Self {
        Self { report }
    }
}

// ─── Protocol constants ───────────────────────────────────────────────────────

pub const PROTOCOL_VERSION: &str = "1.1";

pub mod method {
    pub const INITIALIZE: &str = "initialize";
    pub const INITIALIZED: &str = "initialized";
    pub const PROMPT: &str = "prompt";
    pub const CANCEL: &str = "cancel";
    pub const SHUTDOWN: &str = "shutdown";
    pub const AGENT_EVENT: &str = "agent.event";
    pub const TOOL_APPROVE: &str = "tool.approve";

    // Control plane (protocol 1.1) — client→server requests, only served
    // between turns.
    pub const MODEL_LIST: &str = "model.list";
    pub const MODEL_SET: &str = "model.set";
    pub const THINKING_SET: &str = "thinking.set";
    pub const APPROVAL_GET: &str = "approval.get";
    pub const APPROVAL_SET: &str = "approval.set";
    pub const SYSTEM_PROMPT_SET: &str = "system_prompt.set";
    pub const AGENT_RESET: &str = "agent.reset";
    pub const PLAN_ENTER: &str = "plan.enter";
    pub const PLAN_EXIT: &str = "plan.exit";
    pub const SESSION_SNAPSHOT: &str = "session.snapshot";
    pub const SESSION_RESTORE: &str = "session.restore";
    /// Additive since 1.1: pre-1.1-plus servers answer `METHOD_NOT_FOUND`,
    /// which clients surface as "unsupported" rather than an error.
    pub const CONTEXT_COMPACT: &str = "context.compact";

    /// Returns `true` for control-plane request methods (protocol 1.1).
    ///
    /// While a turn is in flight the server answers these with
    /// [`RpcError::BUSY`](crate::jsonrpc::RpcError::BUSY) instead of
    /// executing them; between turns they are served from the main dispatch
    /// loop. The `cancel` notification (not a control-plane request) remains
    /// the mid-turn-safe way to abort a running turn.
    #[must_use]
    pub fn is_control(method: &str) -> bool {
        matches!(
            method,
            MODEL_LIST
                | MODEL_SET
                | THINKING_SET
                | APPROVAL_GET
                | APPROVAL_SET
                | SYSTEM_PROMPT_SET
                | AGENT_RESET
                | PLAN_ENTER
                | PLAN_EXIT
                | SESSION_SNAPSHOT
                | SESSION_RESTORE
                | CONTEXT_COMPACT
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_advertises_control_plane_capability() {
        // 1.1 added the control-plane methods (model.*, approval.*, plan.*,
        // session.*, thinking.set, system_prompt.set, agent.reset).
        assert_eq!(PROTOCOL_VERSION, "1.1");
    }

    #[test]
    fn control_methods_are_classified_for_busy_handling() {
        for m in [
            method::MODEL_LIST,
            method::MODEL_SET,
            method::THINKING_SET,
            method::APPROVAL_GET,
            method::APPROVAL_SET,
            method::SYSTEM_PROMPT_SET,
            method::AGENT_RESET,
            method::PLAN_ENTER,
            method::PLAN_EXIT,
            method::SESSION_SNAPSHOT,
            method::SESSION_RESTORE,
            method::CONTEXT_COMPACT,
        ] {
            assert!(method::is_control(m), "{m} should be a control method");
        }

        for m in [
            method::INITIALIZE,
            method::INITIALIZED,
            method::PROMPT,
            method::CANCEL,
            method::SHUTDOWN,
            method::AGENT_EVENT,
            method::TOOL_APPROVE,
            "rpc.unknown",
        ] {
            assert!(!method::is_control(m), "{m} should not be a control method");
        }
    }

    #[test]
    fn ack_serializes_as_empty_object() {
        let encoded = serde_json::to_value(Ack::new()).unwrap();
        assert_eq!(encoded, serde_json::json!({}));

        let _decoded: Ack = serde_json::from_value(serde_json::json!({})).unwrap();
    }

    #[test]
    fn approval_params_round_trip_snake_case_modes() {
        let encoded = serde_json::to_value(ApprovalSetParams::new(ApprovalMode::Bypassed)).unwrap();
        assert_eq!(encoded, serde_json::json!({"mode": "bypassed"}));

        let decoded: ApprovalGetResult =
            serde_json::from_value(serde_json::json!({"mode": "smart"})).unwrap();
        assert_eq!(decoded.mode, ApprovalMode::Smart);
    }

    #[test]
    fn session_snapshot_omits_absent_state_and_round_trips() {
        let empty = SessionSnapshot::new(Vec::new(), None);
        let encoded = serde_json::to_value(&empty).unwrap();
        assert!(
            encoded.get("state").is_none(),
            "absent state should stay off the wire"
        );

        let snapshot = SessionSnapshot::new(
            vec![serde_json::json!({"role": "user"})],
            Some(serde_json::json!({"data": {"k": 1}})),
        );
        let decoded: SessionSnapshot =
            serde_json::from_value(serde_json::to_value(&snapshot).unwrap()).unwrap();
        assert_eq!(decoded.messages, snapshot.messages);
        assert_eq!(decoded.state, snapshot.state);

        // `state` may also be omitted entirely in `session.restore` params.
        let decoded: SessionSnapshot =
            serde_json::from_value(serde_json::json!({"messages": []})).unwrap();
        assert!(decoded.state.is_none());
    }

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
        let request = ToolApprovalRequest::new(
            "call-1",
            "write_file",
            serde_json::json!({"path": "notes.md", "content": "ok"}),
            true,
        )
        .with_context(serde_json::json!({"cwd": "/workspace"}));

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
