//! Tests for `client`.
#![cfg(test)]

use swink_agent::{AgentEvent, ToolApproval};
use tokio::{io::duplex, sync::oneshot};

use super::*;
use crate::dto::{PromptResult, ToolApprovalRequestDto};
use crate::jsonrpc::IncomingMessage;

fn make_client_pair() -> (AgentClient, JsonRpcPeer) {
    let (client_read, server_write) = duplex(8192);
    let (server_read, client_write) = duplex(8192);
    let client = AgentClient {
        peer: JsonRpcPeer::new(client_read, client_write),
        approval_handler: None,
    };
    let server = JsonRpcPeer::new(server_read, server_write);
    (client, server)
}

#[tokio::test]
async fn prompt_text_collects_agent_events_until_prompt_response() {
    let (mut client, mut server) = make_client_pair();
    let server_sender = server.sender();
    let (client_done_tx, client_done_rx) = oneshot::channel();

    let server_task = tokio::spawn(async move {
        let incoming = server.recv_incoming().await.unwrap();
        let IncomingMessage::Request { id, method, params } = incoming else {
            panic!("expected prompt request");
        };

        assert_eq!(method, method::PROMPT);
        let params: PromptParams = serde_json::from_value(params.unwrap()).unwrap();
        assert_eq!(params.text, "hello rpc");

        server_sender
            .notify(method::AGENT_EVENT, &AgentEvent::AgentStart)
            .await
            .unwrap();
        server_sender
            .notify(method::AGENT_EVENT, &AgentEvent::TurnStart)
            .await
            .unwrap();
        server_sender
            .respond_ok(
                id,
                PromptResult {
                    turn_id: "1".into(),
                },
            )
            .await
            .unwrap();
        let _ = client_done_rx.await;
    });

    let events = client.prompt_text("hello rpc").await.unwrap();
    let _ = client_done_tx.send(());

    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], AgentEvent::AgentStart));
    assert!(matches!(events[1], AgentEvent::TurnStart));
    server_task.await.unwrap();
}

#[tokio::test]
async fn prompt_text_with_streams_events_through_the_callback() {
    let (mut client, mut server) = make_client_pair();
    let server_sender = server.sender();
    let (client_done_tx, client_done_rx) = oneshot::channel();

    let server_task = tokio::spawn(async move {
        let incoming = server.recv_incoming().await.unwrap();
        let IncomingMessage::Request { id, method, .. } = incoming else {
            panic!("expected prompt request");
        };
        assert_eq!(method, method::PROMPT);

        server_sender
            .notify(method::AGENT_EVENT, &AgentEvent::AgentStart)
            .await
            .unwrap();
        server_sender
            .notify(method::AGENT_EVENT, &AgentEvent::TurnStart)
            .await
            .unwrap();
        server_sender
            .respond_ok(
                id,
                PromptResult {
                    turn_id: "6".into(),
                },
            )
            .await
            .unwrap();
        let _ = client_done_rx.await;
    });

    let mut seen = Vec::new();
    client
        .prompt_text_with("hello streaming", |event| seen.push(event))
        .await
        .unwrap();
    let _ = client_done_tx.send(());

    assert_eq!(seen.len(), 2);
    assert!(matches!(seen[0], AgentEvent::AgentStart));
    assert!(matches!(seen[1], AgentEvent::TurnStart));
    server_task.await.unwrap();
}

#[tokio::test]
async fn prompt_text_answers_tool_approval_requests() {
    let (client, mut server) = make_client_pair();
    let mut client = client.with_approval_handler(|req| {
        assert_eq!(req.tool_call_id, "call-1");
        assert_eq!(req.tool_name, "dangerous_tool");
        ToolApproval::Rejected
    });
    let server_sender = server.sender();
    let (client_done_tx, client_done_rx) = oneshot::channel();

    let server_task = tokio::spawn(async move {
        let incoming = server.recv_incoming().await.unwrap();
        let IncomingMessage::Request { id, method, .. } = incoming else {
            panic!("expected prompt request");
        };
        assert_eq!(method, method::PROMPT);

        let approval = server_sender
            .request::<_, ToolApprovalDto>(
                method::TOOL_APPROVE,
                &ToolApprovalRequestDto {
                    id: "call-1".into(),
                    name: "dangerous_tool".into(),
                    arguments: serde_json::json!({"path": "/tmp/example"}),
                    requires_approval: true,
                    context: None,
                },
            )
            .await
            .unwrap();
        assert!(matches!(approval, ToolApprovalDto::Rejected));

        server_sender
            .respond_ok(
                id,
                PromptResult {
                    turn_id: "2".into(),
                },
            )
            .await
            .unwrap();
        let _ = client_done_rx.await;
    });

    let events = client.prompt_text("run tool").await.unwrap();
    let _ = client_done_tx.send(());

    assert!(events.is_empty());
    server_task.await.unwrap();
}

#[tokio::test]
async fn prompt_text_auto_approves_tool_approval_requests_without_handler() {
    let (mut client, mut server) = make_client_pair();
    let server_sender = server.sender();
    let (client_done_tx, client_done_rx) = oneshot::channel();

    let server_task = tokio::spawn(async move {
        let incoming = server.recv_incoming().await.unwrap();
        let IncomingMessage::Request { id, method, .. } = incoming else {
            panic!("expected prompt request");
        };
        assert_eq!(method, method::PROMPT);

        let approval = server_sender
            .request::<_, ToolApprovalDto>(
                method::TOOL_APPROVE,
                &ToolApprovalRequestDto {
                    id: "call-1".into(),
                    name: "dangerous_tool".into(),
                    arguments: serde_json::json!({"path": "/tmp/example"}),
                    requires_approval: true,
                    context: None,
                },
            )
            .await
            .unwrap();
        assert!(matches!(approval, ToolApprovalDto::Approved));

        server_sender
            .respond_ok(
                id,
                PromptResult {
                    turn_id: "3".into(),
                },
            )
            .await
            .unwrap();
        let _ = client_done_rx.await;
    });

    let events = client.prompt_text("run tool").await.unwrap();
    let _ = client_done_tx.send(());

    assert!(events.is_empty());
    server_task.await.unwrap();
}

#[tokio::test]
async fn prompt_text_rejects_malformed_tool_approval_requests_with_handler() {
    let (client, mut server) = make_client_pair();
    let mut client = client.with_approval_handler(|_| {
        panic!("malformed approval requests must not reach the handler");
    });
    let server_sender = server.sender();
    let (client_done_tx, client_done_rx) = oneshot::channel();

    let server_task = tokio::spawn(async move {
        let incoming = server.recv_incoming().await.unwrap();
        let IncomingMessage::Request { id, method, .. } = incoming else {
            panic!("expected prompt request");
        };
        assert_eq!(method, method::PROMPT);

        let approval = server_sender
            .request::<_, ToolApprovalDto>(method::TOOL_APPROVE, &serde_json::json!({}))
            .await
            .unwrap();
        assert!(matches!(approval, ToolApprovalDto::Rejected));

        server_sender
            .respond_ok(
                id,
                PromptResult {
                    turn_id: "4".into(),
                },
            )
            .await
            .unwrap();
        let _ = client_done_rx.await;
    });

    let events = client.prompt_text("run tool").await.unwrap();
    let _ = client_done_tx.send(());

    assert!(events.is_empty());
    server_task.await.unwrap();
}

#[tokio::test]
async fn prompt_text_replies_method_not_found_to_unknown_requests() {
    let (mut client, mut server) = make_client_pair();
    let server_sender = server.sender();
    let (client_done_tx, client_done_rx) = oneshot::channel();

    let server_task = tokio::spawn(async move {
        let incoming = server.recv_incoming().await.unwrap();
        let IncomingMessage::Request { id, method, .. } = incoming else {
            panic!("expected prompt request");
        };
        assert_eq!(method, method::PROMPT);

        let err = server_sender
            .request::<_, serde_json::Value>("server.unknown", &serde_json::json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.code, RpcError::METHOD_NOT_FOUND);
        assert_eq!(err.message, "method not found: server.unknown");

        server_sender
            .respond_ok(
                id,
                PromptResult {
                    turn_id: "5".into(),
                },
            )
            .await
            .unwrap();
        let _ = client_done_rx.await;
    });

    let events = client
        .prompt_text("run unknown server request")
        .await
        .unwrap();
    let _ = client_done_tx.send(());

    assert!(events.is_empty());
    server_task.await.unwrap();
}

#[tokio::test]
async fn cancel_sends_cancel_notification() {
    let (client, mut server) = make_client_pair();

    client.cancel().await.unwrap();

    let Some(IncomingMessage::Notification { method, params: _ }) = server.recv_incoming().await
    else {
        panic!("expected cancel notification");
    };
    assert_eq!(method, method::CANCEL);
}

#[tokio::test]
async fn shutdown_sends_shutdown_notification() {
    let (client, mut server) = make_client_pair();

    client.shutdown().await.unwrap();

    let Some(IncomingMessage::Notification { method, params: _ }) = server.recv_incoming().await
    else {
        panic!("expected shutdown notification");
    };
    assert_eq!(method, method::SHUTDOWN);
}

#[tokio::test]
// One scripted server conversation exercising every control helper in
// order; splitting it would duplicate the fake-server dispatch loop.
#[allow(clippy::too_many_lines)]
async fn control_helpers_round_trip_typed_requests() {
    let (client, mut server) = make_client_pair();
    let server_sender = server.sender();

    let server_task = tokio::spawn(async move {
        let mut seen = Vec::new();
        while let Some(incoming) = server.recv_incoming().await {
            let IncomingMessage::Request {
                id,
                method: m,
                params,
            } = incoming
            else {
                panic!("expected only requests, got {incoming:?}");
            };
            seen.push(m.clone());
            match m.as_str() {
                method::MODEL_LIST => {
                    server_sender
                        .respond_ok(
                            id,
                            ModelListResult::new(
                                vec![ModelSpec::new("test", "alt-model")],
                                ModelSpec::new("test", "test-model"),
                            ),
                        )
                        .await
                        .unwrap();
                }
                method::MODEL_SET => {
                    let p: ModelSetParams = serde_json::from_value(params.unwrap()).unwrap();
                    assert_eq!(p.model.model_id, "alt-model");
                    server_sender.respond_ok(id, Ack::new()).await.unwrap();
                }
                method::THINKING_SET => {
                    let p: ThinkingSetParams = serde_json::from_value(params.unwrap()).unwrap();
                    assert_eq!(p.level, ThinkingLevel::High);
                    server_sender.respond_ok(id, Ack::new()).await.unwrap();
                }
                method::APPROVAL_GET => {
                    server_sender
                        .respond_ok(id, ApprovalGetResult::new(ApprovalMode::Bypassed))
                        .await
                        .unwrap();
                }
                method::APPROVAL_SET => {
                    let p: ApprovalSetParams = serde_json::from_value(params.unwrap()).unwrap();
                    assert_eq!(p.mode, ApprovalMode::Enabled);
                    server_sender.respond_ok(id, Ack::new()).await.unwrap();
                }
                method::SYSTEM_PROMPT_SET => {
                    let p: SystemPromptSetParams = serde_json::from_value(params.unwrap()).unwrap();
                    assert_eq!(p.prompt, "fresh prompt");
                    server_sender.respond_ok(id, Ack::new()).await.unwrap();
                }
                method::AGENT_RESET | method::PLAN_ENTER | method::PLAN_EXIT => {
                    server_sender.respond_ok(id, Ack::new()).await.unwrap();
                }
                method::SESSION_SNAPSHOT => {
                    server_sender
                        .respond_ok(
                            id,
                            SessionSnapshot::new(
                                vec![serde_json::json!({"role": "user"})],
                                Some(serde_json::json!({"data": {}})),
                            ),
                        )
                        .await
                        .unwrap();
                }
                method::SESSION_RESTORE => {
                    let p: SessionSnapshot = serde_json::from_value(params.unwrap()).unwrap();
                    assert_eq!(p.messages.len(), 1);
                    server_sender.respond_ok(id, Ack::new()).await.unwrap();
                }
                method::CONTEXT_COMPACT => {
                    server_sender
                        .respond_ok(
                            id,
                            crate::dto::CompactResult::new(Some(
                                swink_agent::CompactionReport::new(3, 9_000, 2_000, true),
                            )),
                        )
                        .await
                        .unwrap();
                }
                other => panic!("unexpected control method: {other}"),
            }
        }
        seen
    });

    let listed = client.list_models().await.unwrap();
    assert_eq!(listed.available.len(), 1);
    assert_eq!(listed.current.model_id, "test-model");

    client
        .set_model(ModelSpec::new("test", "alt-model"))
        .await
        .unwrap();
    client
        .set_thinking_level(ThinkingLevel::High)
        .await
        .unwrap();
    assert_eq!(
        client.approval_mode().await.unwrap(),
        ApprovalMode::Bypassed
    );
    client
        .set_approval_mode(ApprovalMode::Enabled)
        .await
        .unwrap();
    client.set_system_prompt("fresh prompt").await.unwrap();
    client.reset().await.unwrap();
    client.enter_plan_mode().await.unwrap();
    client.exit_plan_mode().await.unwrap();

    let snapshot = client.session_snapshot().await.unwrap();
    assert_eq!(snapshot.messages.len(), 1);
    assert!(snapshot.state.is_some());
    client.session_restore(snapshot).await.unwrap();

    let compacted = client.compact().await.unwrap();
    let report = compacted.report.expect("mock server returns a report");
    assert_eq!(report.dropped_count, 3);

    drop(client);
    let seen = server_task.await.unwrap();
    assert_eq!(
        seen,
        vec![
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
        ]
    );
}

#[tokio::test]
async fn sender_exposes_peer_sender_for_out_of_band_control() {
    let (client, mut server) = make_client_pair();

    // A cloned sender can issue notifications (e.g. `cancel`) from
    // another task while the client itself is busy driving a turn.
    let sender = client.sender();
    sender
        .notify(method::CANCEL, &serde_json::Value::Null)
        .await
        .unwrap();

    let Some(IncomingMessage::Notification { method: m, .. }) = server.recv_incoming().await else {
        panic!("expected cancel notification");
    };
    assert_eq!(m, method::CANCEL);
}

#[cfg(not(unix))]
#[tokio::test]
async fn connect_reports_unix_transport_unavailable_on_non_unix_hosts() {
    let Err(err) = AgentClient::connect("unused.sock").await else {
        panic!("non-Unix client connect should fail");
    };

    assert_eq!(err.code, RpcError::UNAVAILABLE);
    assert!(
        err.message.contains("Unix socket transport"),
        "unexpected error message: {}",
        err.message
    );
}
