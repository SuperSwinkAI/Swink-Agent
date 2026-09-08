//! Tests for `message`.
#![cfg(test)]

use super::*;

#[test]
fn request_id_round_trips_numbers_and_strings() {
    let numeric = serde_json::to_value(RequestId::Number(42)).unwrap();
    let string = serde_json::to_value(RequestId::Str("call-42".into())).unwrap();

    assert_eq!(numeric, serde_json::json!(42));
    assert_eq!(string, serde_json::json!("call-42"));

    assert_eq!(
        serde_json::from_value::<RequestId>(numeric).unwrap(),
        RequestId::Number(42)
    );
    assert_eq!(
        serde_json::from_value::<RequestId>(string).unwrap(),
        RequestId::Str("call-42".into())
    );
    assert_eq!(RequestId::Number(42).to_string(), "42");
    assert_eq!(RequestId::Str("call-42".into()).to_string(), "call-42");
}

#[test]
fn raw_message_constructors_serialize_minimal_jsonrpc_envelopes() {
    let request = serde_json::to_value(RawMessage::request(
        RequestId::Number(1),
        "prompt",
        serde_json::json!({"text": "hello"}),
    ))
    .unwrap();
    let notification =
        serde_json::to_value(RawMessage::notification("cancel", serde_json::Value::Null)).unwrap();
    let success = serde_json::to_value(RawMessage::success(
        RequestId::Str("req-2".into()),
        serde_json::json!({"ok": true}),
    ))
    .unwrap();
    let error = serde_json::to_value(RawMessage::error_response(
        RequestId::Number(3),
        RpcError::method_not_found("missing"),
    ))
    .unwrap();

    assert_eq!(
        request,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "prompt",
            "params": {"text": "hello"}
        })
    );
    assert_eq!(
        notification,
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "cancel",
            "params": null
        })
    );
    assert_eq!(
        success,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": "req-2",
            "result": {"ok": true}
        })
    );
    assert_eq!(
        error,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "error": {
                "code": RpcError::METHOD_NOT_FOUND,
                "message": "method not found: missing"
            }
        })
    );
}

#[test]
fn raw_message_classifies_wire_shapes() {
    let request = RawMessage::request(
        RequestId::Number(1),
        "prompt",
        serde_json::json!({"text": "hello"}),
    );
    let notification = RawMessage::notification("shutdown", serde_json::Value::Null);
    let response = RawMessage::success(
        RequestId::Str("req-1".into()),
        serde_json::json!({"ok": true}),
    );
    let invalid = RawMessage {
        jsonrpc: "2.0".into(),
        id: None,
        method: None,
        params: None,
        result: None,
        error: None,
    };

    match request.classify() {
        MessageKind::Request { id, method } => {
            assert_eq!(id, &RequestId::Number(1));
            assert_eq!(method, "prompt");
        }
        other => panic!("expected request, got {other:?}"),
    }

    match notification.classify() {
        MessageKind::Notification { method } => assert_eq!(method, "shutdown"),
        other => panic!("expected notification, got {other:?}"),
    }

    match response.classify() {
        MessageKind::Response { id } => assert_eq!(id, &RequestId::Str("req-1".into())),
        other => panic!("expected response, got {other:?}"),
    }

    assert!(matches!(invalid.classify(), MessageKind::Invalid));
}
