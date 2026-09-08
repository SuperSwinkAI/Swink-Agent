//! Tests for `dto`.
#![cfg(test)]

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
    let approved = serde_json::to_value(ToolApprovalDto::from(&ToolApproval::Approved)).unwrap();
    let rejected = serde_json::to_value(ToolApprovalDto::from(&ToolApproval::Rejected)).unwrap();
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
