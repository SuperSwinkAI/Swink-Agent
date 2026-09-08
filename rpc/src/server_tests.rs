//! Tests for `server`.
#![cfg(test)]

use std::io::ErrorKind;
use std::sync::Arc;
use std::time::Duration;

use swink_agent::{
    AgentEvent, AgentOptions, AgentTool, ApprovalMode, LlmMessage, ModelSpec, StreamFn,
    ThinkingLevel,
};
use tokio::io::duplex;

use super::*;
use crate::dto::{
    Ack, ApprovalGetResult, ApprovalSetParams, ClientInfo, InitializeParams, ModelListResult,
    ModelSetParams, PROTOCOL_VERSION, PromptParams, PromptResult, SessionSnapshot,
    SystemPromptSetParams, ThinkingSetParams, ToolApprovalDto, ToolApprovalRequestDto, method,
};
use crate::jsonrpc::{IncomingMessage, JsonRpcPeer};

fn make_peer_pair() -> (JsonRpcPeer, JsonRpcPeer) {
    let (client_read, server_write) = duplex(8192);
    let (server_read, client_write) = duplex(8192);
    (
        JsonRpcPeer::new(client_read, client_write),
        JsonRpcPeer::new(server_read, server_write),
    )
}

fn test_agent_options(response: &'static str) -> AgentOptions {
    let stream_fn: Arc<dyn StreamFn> = Arc::new(
        swink_agent::testing::SimpleMockStreamFn::from_text(response),
    );
    AgentOptions::new(
        "test system",
        swink_agent::testing::default_model(),
        stream_fn,
        swink_agent::testing::default_convert,
    )
}

fn approval_blocking_agent_options() -> AgentOptions {
    let stream_fn: Arc<dyn StreamFn> = Arc::new(swink_agent::testing::MockStreamFn::new(vec![
        swink_agent::testing::tool_call_events("call-1", "blocked_tool", r"{}"),
    ]));
    let tool =
        Arc::new(swink_agent::testing::MockTool::new("blocked_tool").with_requires_approval(true));

    AgentOptions::new(
        "test system",
        swink_agent::testing::default_model(),
        stream_fn,
        swink_agent::testing::default_convert,
    )
    .with_tools(vec![tool as Arc<dyn AgentTool>])
}

#[test]
fn bind_rejects_existing_socket_path_without_force() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("swink.sock");
    std::fs::write(&path, b"stale socket placeholder").unwrap();

    let err = match AgentServer::bind(&path, || Ok(test_agent_options("unused"))) {
        Ok(_) => panic!("bind should reject existing socket path"),
        Err(err) => err,
    };

    assert_eq!(err.kind(), ErrorKind::AlreadyExists);
    assert!(
        err.to_string().contains("remove it or pass --force"),
        "unexpected bind error: {err}"
    );
    assert!(
        path.exists(),
        "bind without force must not remove the existing path"
    );
}

#[test]
fn bind_force_removes_existing_stale_socket_path() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("swink.sock");
    std::fs::write(&path, b"stale socket placeholder").unwrap();

    let _server = AgentServer::bind_force(&path, || Ok(test_agent_options("unused")));

    assert!(
        !path.exists(),
        "bind_force should remove a stale socket path before serving"
    );
}

async fn initialize(peer: &mut JsonRpcPeer) {
    peer.sender()
        .notify(
            method::INITIALIZE,
            &InitializeParams {
                protocol_version: PROTOCOL_VERSION.into(),
                client: ClientInfo {
                    name: "test-client".into(),
                    version: "0.1.0".into(),
                },
            },
        )
        .await
        .unwrap();

    let Some(IncomingMessage::Notification { method: m, .. }) = peer.recv_incoming().await else {
        panic!("expected initialized notification");
    };
    assert_eq!(m, method::INITIALIZED);
}

#[tokio::test]
async fn run_session_streams_prompt_events_and_turn_response() {
    let (mut client, mut server) = make_peer_pair();

    let server_task = tokio::spawn(async move {
        run_session(&mut server, &|| {
            Ok(test_agent_options("hello from rpc server"))
        })
        .await
        .unwrap();
    });

    initialize(&mut client).await;

    let sender = client.sender();
    let params = PromptParams {
        text: "hello rpc".into(),
        session_id: None,
    };
    let prompt = sender.request::<_, PromptResult>(method::PROMPT, &params);
    let mut prompt = std::pin::pin!(prompt);
    let mut events = Vec::new();
    let result = loop {
        tokio::select! {
            result = &mut prompt => {
                let result = result.unwrap();
                while let Some(incoming) = client.try_recv_incoming() {
                    collect_agent_event(incoming, &mut events);
                }
                break result;
            }
            incoming = client.recv_incoming() => {
                let incoming = incoming.expect("server should stay connected while prompt runs");
                collect_agent_event(incoming, &mut events);
            }
        }
    };

    assert!(!result.turn_id.is_empty());
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::TurnStart)),
        "server should stream turn lifecycle events"
    );
    assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::MessageEnd { message } if message
                    .content
                    .iter()
                    .any(|block| matches!(block, swink_agent::ContentBlock::Text { text } if text == "hello from rpc server")))),
            "server should stream the assistant response body"
        );

    client
        .sender()
        .notify(method::SHUTDOWN, &serde_json::Value::Null)
        .await
        .unwrap();
    server_task.await.unwrap();
}

#[tokio::test]
async fn run_session_rejects_invalid_prompt_params_without_ending_session() {
    let (mut client, mut server) = make_peer_pair();

    let server_task = tokio::spawn(async move {
        run_session(&mut server, &|| Ok(test_agent_options("valid follow-up")))
            .await
            .unwrap();
    });

    initialize(&mut client).await;

    let sender = client.sender();
    let err = sender
        .request::<_, PromptResult>(
            method::PROMPT,
            &serde_json::json!({
                "session_id": "missing-text"
            }),
        )
        .await
        .unwrap_err();

    assert_eq!(err.code, crate::jsonrpc::RpcError::INVALID_REQUEST);
    assert!(
        err.message.contains("missing or invalid prompt params"),
        "unexpected prompt error: {}",
        err.message
    );

    let params = PromptParams {
        text: "recover after invalid params".into(),
        session_id: None,
    };
    let prompt = sender.request::<_, PromptResult>(method::PROMPT, &params);
    let mut prompt = std::pin::pin!(prompt);
    let mut events = Vec::new();
    let result = loop {
        tokio::select! {
            result = &mut prompt => {
                let result = result.unwrap();
                while let Some(incoming) = client.try_recv_incoming() {
                    collect_agent_event(incoming, &mut events);
                }
                break result;
            }
            incoming = client.recv_incoming() => {
                let incoming = incoming.expect("server should stay connected after rejecting invalid prompt");
                collect_agent_event(incoming, &mut events);
            }
        }
    };

    assert!(!result.turn_id.is_empty());
    assert!(
            events.iter().any(|event| matches!(
                event,
                AgentEvent::MessageEnd { message } if message
                    .content
                    .iter()
                    .any(|block| matches!(block, swink_agent::ContentBlock::Text { text } if text == "valid follow-up"))
            )),
            "server should continue serving valid prompts after an invalid request"
        );

    client
        .sender()
        .notify(method::SHUTDOWN, &serde_json::Value::Null)
        .await
        .unwrap();
    server_task.await.unwrap();
}

#[tokio::test]
async fn run_session_rejects_unknown_requests_without_ending_session() {
    let (mut client, mut server) = make_peer_pair();

    let server_task = tokio::spawn(async move {
        run_session(&mut server, &|| Ok(test_agent_options("unused")))
            .await
            .unwrap();
    });

    initialize(&mut client).await;

    let sender = client.sender();
    let err = sender
        .request::<_, serde_json::Value>("rpc.unknown", &serde_json::json!({}))
        .await
        .unwrap_err();

    assert_eq!(err.code, crate::jsonrpc::RpcError::METHOD_NOT_FOUND);
    assert_eq!(err.message, "method not found: rpc.unknown");

    sender
        .notify(method::SHUTDOWN, &serde_json::Value::Null)
        .await
        .unwrap();
    server_task.await.unwrap();
}

#[tokio::test]
async fn run_session_ignores_idle_cancel_without_ending_session() {
    let (mut client, mut server) = make_peer_pair();

    let server_task = tokio::spawn(async move {
        run_session(&mut server, &|| Ok(test_agent_options("after idle cancel")))
            .await
            .unwrap();
    });

    initialize(&mut client).await;

    let sender = client.sender();
    sender
        .notify(method::CANCEL, &serde_json::Value::Null)
        .await
        .unwrap();

    let params = PromptParams {
        text: "still accepts prompts".into(),
        session_id: None,
    };
    let prompt = sender.request::<_, PromptResult>(method::PROMPT, &params);
    let mut prompt = std::pin::pin!(prompt);
    let mut events = Vec::new();
    let result = loop {
        tokio::select! {
            result = &mut prompt => {
                let result = result.unwrap();
                while let Some(incoming) = client.try_recv_incoming() {
                    collect_agent_event(incoming, &mut events);
                }
                break result;
            }
            incoming = client.recv_incoming() => {
                let incoming = incoming.expect("server should stay connected after idle cancel");
                collect_agent_event(incoming, &mut events);
            }
        }
    };

    assert!(!result.turn_id.is_empty());
    assert!(
            events.iter().any(|event| matches!(
                event,
                AgentEvent::MessageEnd { message } if message
                    .content
                    .iter()
                    .any(|block| matches!(block, swink_agent::ContentBlock::Text { text } if text == "after idle cancel"))
            )),
            "server should keep serving prompts after an idle cancel notification"
        );

    sender
        .notify(method::SHUTDOWN, &serde_json::Value::Null)
        .await
        .unwrap();
    server_task.await.unwrap();
}

#[tokio::test]
async fn run_session_shutdown_during_prompt_ends_session() {
    let (mut client, mut server) = make_peer_pair();

    let server_task = tokio::spawn(async move {
        run_session(&mut server, &|| Ok(approval_blocking_agent_options()))
            .await
            .unwrap();
    });

    initialize(&mut client).await;

    let sender = client.sender();
    let params = PromptParams {
        text: "start a long prompt".into(),
        session_id: None,
    };
    let prompt_sender = sender.clone();
    let prompt_task = tokio::spawn(async move {
        prompt_sender
            .request::<_, PromptResult>(method::PROMPT, &params)
            .await
    });
    let mut prompt_task = std::pin::pin!(prompt_task);

    loop {
        tokio::select! {
            result = &mut prompt_task => {
                panic!("prompt resolved before tool approval request: {result:?}");
            }
            incoming = client.recv_incoming() => {
                match incoming.expect("server should request approval before shutdown") {
                    IncomingMessage::Request { method: m, .. } if m == method::TOOL_APPROVE => break,
                    IncomingMessage::Notification { method: m, .. } if m == method::AGENT_EVENT => {}
                    other => panic!("unexpected message while awaiting tool approval: {other:?}"),
                }
            }
        }
    }

    sender
        .notify(method::SHUTDOWN, &serde_json::Value::Null)
        .await
        .unwrap();

    let err = prompt_task.await.unwrap().unwrap_err();
    assert_eq!(err.code, crate::jsonrpc::RpcError::DISCONNECTED);

    tokio::time::timeout(Duration::from_secs(1), server_task)
        .await
        .expect("shutdown during a prompt should end the session")
        .unwrap();
}

#[tokio::test]
async fn run_session_round_trips_tool_approval_during_prompt() {
    let (mut client, mut server) = make_peer_pair();
    let stream_fn: Arc<dyn StreamFn> = Arc::new(swink_agent::testing::MockStreamFn::new(vec![
        swink_agent::testing::tool_call_events(
            "call-1",
            "dangerous_tool",
            r#"{"path":"/tmp/example"}"#,
        ),
        swink_agent::testing::text_only_events("done after approval"),
    ]));
    let tool = Arc::new(
        swink_agent::testing::MockTool::new("dangerous_tool").with_requires_approval(true),
    );
    let executed_tool = Arc::clone(&tool);

    let server_task = tokio::spawn(async move {
        let factory = || {
            Ok(AgentOptions::new(
                "test system",
                swink_agent::testing::default_model(),
                Arc::clone(&stream_fn),
                swink_agent::testing::default_convert,
            )
            .with_tools(vec![Arc::clone(&tool) as Arc<dyn AgentTool>]))
        };

        run_session(&mut server, &factory).await.unwrap();
    });

    initialize(&mut client).await;

    let sender = client.sender();
    let params = PromptParams {
        text: "run approved tool".into(),
        session_id: None,
    };
    let prompt = sender.request::<_, PromptResult>(method::PROMPT, &params);
    let mut prompt = std::pin::pin!(prompt);
    let mut events = Vec::new();
    let mut approvals = 0;
    let result = loop {
        tokio::select! {
            result = &mut prompt => {
                let result = result.unwrap();
                while let Some(incoming) = client.try_recv_incoming() {
                    handle_prompt_incoming(incoming, &sender, &mut events, &mut approvals)
                        .await;
                }
                break result;
            }
            incoming = client.recv_incoming() => {
                let incoming = incoming.expect("server should stay connected while prompt runs");
                handle_prompt_incoming(incoming, &sender, &mut events, &mut approvals).await;
            }
        }
    };

    assert!(!result.turn_id.is_empty());
    assert_eq!(approvals, 1);
    assert!(executed_tool.was_executed());
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolApprovalResolved { approved, .. } if *approved
        )),
        "server should continue the turn after receiving approval"
    );

    client
        .sender()
        .notify(method::SHUTDOWN, &serde_json::Value::Null)
        .await
        .unwrap();
    server_task.await.unwrap();
}

#[tokio::test]
async fn run_session_rejects_protocol_version_mismatch() {
    let (mut client, mut server) = make_peer_pair();

    let server_task = tokio::spawn(async move {
        run_session(&mut server, &|| Ok(test_agent_options("unused")))
            .await
            .unwrap_err()
    });

    client
        .sender()
        .notify(
            method::INITIALIZE,
            &InitializeParams {
                protocol_version: "0.9".into(),
                client: ClientInfo::default(),
            },
        )
        .await
        .unwrap();

    let err = server_task.await.unwrap();
    assert_eq!(err.code, crate::jsonrpc::RpcError::PROTOCOL_MISMATCH);
    assert!(client.try_recv_incoming().is_none());
}

#[tokio::test]
async fn run_session_serves_model_and_thinking_control_requests_between_turns() {
    let (mut client, mut server) = make_peer_pair();

    let server_task = tokio::spawn(async move {
        run_session(&mut server, &|| Ok(test_agent_options("unused")))
            .await
            .unwrap();
    });

    initialize(&mut client).await;
    let sender = client.sender();

    let listed: ModelListResult = sender
        .request(method::MODEL_LIST, &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(listed.current, swink_agent::testing::default_model());
    assert_eq!(
        listed.available,
        vec![swink_agent::testing::default_model()],
        "the primary model is always listed, even with no extra models registered"
    );

    let next = ModelSpec::new("test", "next-model");
    let _: Ack = sender
        .request(method::MODEL_SET, &ModelSetParams::new(next))
        .await
        .unwrap();
    let _: Ack = sender
        .request(
            method::THINKING_SET,
            &ThinkingSetParams::new(ThinkingLevel::High),
        )
        .await
        .unwrap();

    let listed: ModelListResult = sender
        .request(method::MODEL_LIST, &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(listed.current.model_id, "next-model");
    assert_eq!(listed.current.thinking_level, ThinkingLevel::High);

    sender
        .notify(method::SHUTDOWN, &serde_json::Value::Null)
        .await
        .unwrap();
    server_task.await.unwrap();
}

#[tokio::test]
async fn run_session_round_trips_approval_mode_and_acks_system_prompt() {
    let (mut client, mut server) = make_peer_pair();

    let server_task = tokio::spawn(async move {
        run_session(&mut server, &|| Ok(test_agent_options("unused")))
            .await
            .unwrap();
    });

    initialize(&mut client).await;
    let sender = client.sender();

    let current: ApprovalGetResult = sender
        .request(method::APPROVAL_GET, &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(current.mode, ApprovalMode::Smart, "Smart is the default");

    let _: Ack = sender
        .request(
            method::APPROVAL_SET,
            &ApprovalSetParams::new(ApprovalMode::Bypassed),
        )
        .await
        .unwrap();

    let current: ApprovalGetResult = sender
        .request(method::APPROVAL_GET, &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(current.mode, ApprovalMode::Bypassed);

    let _: Ack = sender
        .request(
            method::SYSTEM_PROMPT_SET,
            &SystemPromptSetParams::new("you are a replaced prompt"),
        )
        .await
        .unwrap();

    sender
        .notify(method::SHUTDOWN, &serde_json::Value::Null)
        .await
        .unwrap();
    server_task.await.unwrap();
}

#[tokio::test]
async fn run_session_rejects_double_plan_enter_and_unpaired_plan_exit() {
    let (mut client, mut server) = make_peer_pair();

    let server_task = tokio::spawn(async move {
        run_session(&mut server, &|| Ok(test_agent_options("unused")))
            .await
            .unwrap();
    });

    initialize(&mut client).await;
    let sender = client.sender();

    let _: Ack = sender
        .request(method::PLAN_ENTER, &serde_json::json!({}))
        .await
        .unwrap();

    let err = sender
        .request::<_, Ack>(method::PLAN_ENTER, &serde_json::json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.code, crate::jsonrpc::RpcError::INVALID_REQUEST);
    assert!(
        err.message.contains("already in plan mode"),
        "unexpected plan.enter error: {}",
        err.message
    );

    let _: Ack = sender
        .request(method::PLAN_EXIT, &serde_json::json!({}))
        .await
        .unwrap();

    let err = sender
        .request::<_, Ack>(method::PLAN_EXIT, &serde_json::json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.code, crate::jsonrpc::RpcError::INVALID_REQUEST);
    assert!(
        err.message.contains("not in plan mode"),
        "unexpected plan.exit error: {}",
        err.message
    );

    sender
        .notify(method::SHUTDOWN, &serde_json::Value::Null)
        .await
        .unwrap();
    server_task.await.unwrap();
}

#[tokio::test]
async fn run_session_snapshot_reset_restore_round_trips_messages_and_state() {
    let (mut client, mut server) = make_peer_pair();

    let server_task = tokio::spawn(async move {
        run_session(&mut server, &|| Ok(test_agent_options("snapshot me")))
            .await
            .unwrap();
    });

    initialize(&mut client).await;
    let sender = client.sender();

    // Run one turn so the transcript is non-empty.
    let params = PromptParams {
        text: "hello snapshot".into(),
        session_id: None,
    };
    let result: PromptResult = sender.request(method::PROMPT, &params).await.unwrap();
    assert!(!result.turn_id.is_empty());
    // Drain the buffered agent.event notifications from the turn.
    while client.try_recv_incoming().is_some() {}

    let snapshot: SessionSnapshot = sender
        .request(method::SESSION_SNAPSHOT, &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(
        snapshot.messages.len(),
        2,
        "one user + one assistant message expected"
    );
    // Messages use the memory-JSONL representation: raw LlmMessage JSON.
    let first: LlmMessage = serde_json::from_value(snapshot.messages[0].clone()).unwrap();
    assert!(matches!(first, LlmMessage::User(_)));
    assert!(snapshot.state.is_some());

    let _: Ack = sender
        .request(method::AGENT_RESET, &serde_json::json!({}))
        .await
        .unwrap();
    let cleared: SessionSnapshot = sender
        .request(method::SESSION_SNAPSHOT, &serde_json::json!({}))
        .await
        .unwrap();
    assert!(
        cleared.messages.is_empty(),
        "agent.reset should clear the transcript"
    );

    // Restore the original snapshot, but with explicit session state.
    let restore = SessionSnapshot::new(
        snapshot.messages.clone(),
        Some(serde_json::json!({"data": {"favorite": 42}})),
    );
    let _: Ack = sender
        .request(method::SESSION_RESTORE, &restore)
        .await
        .unwrap();

    let restored: SessionSnapshot = sender
        .request(method::SESSION_SNAPSHOT, &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(restored.messages, snapshot.messages);
    assert_eq!(
        restored.state,
        Some(serde_json::json!({"data": {"favorite": 42}}))
    );

    sender
        .notify(method::SHUTDOWN, &serde_json::Value::Null)
        .await
        .unwrap();
    server_task.await.unwrap();
}

#[tokio::test]
async fn run_session_rejects_malformed_session_restore_messages() {
    let (mut client, mut server) = make_peer_pair();

    let server_task = tokio::spawn(async move {
        run_session(&mut server, &|| Ok(test_agent_options("unused")))
            .await
            .unwrap();
    });

    initialize(&mut client).await;
    let sender = client.sender();

    let err = sender
        .request::<_, Ack>(
            method::SESSION_RESTORE,
            &SessionSnapshot::new(vec![serde_json::json!({"not": "a message"})], None),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, crate::jsonrpc::RpcError::INVALID_REQUEST);
    assert!(
        err.message.contains("session.restore"),
        "unexpected restore error: {}",
        err.message
    );

    sender
        .notify(method::SHUTDOWN, &serde_json::Value::Null)
        .await
        .unwrap();
    server_task.await.unwrap();
}

#[tokio::test]
async fn run_prompt_answers_control_requests_with_busy_while_turn_in_flight() {
    let (mut client, mut server) = make_peer_pair();

    let server_task = tokio::spawn(async move {
        run_session(&mut server, &|| Ok(approval_blocking_agent_options()))
            .await
            .unwrap();
    });

    initialize(&mut client).await;
    let sender = client.sender();

    let params = PromptParams {
        text: "start a long prompt".into(),
        session_id: None,
    };
    let prompt_sender = sender.clone();
    let prompt_task = tokio::spawn(async move {
        prompt_sender
            .request::<_, PromptResult>(method::PROMPT, &params)
            .await
    });

    // Wait for the server's tool.approve request — the turn is now
    // provably in flight, blocked on our approval decision.
    let approval_id = loop {
        match client
            .recv_incoming()
            .await
            .expect("server should stay connected while prompt runs")
        {
            IncomingMessage::Request { id, method: m, .. } if m == method::TOOL_APPROVE => {
                break id;
            }
            IncomingMessage::Notification { method: m, .. } if m == method::AGENT_EVENT => {}
            other => panic!("unexpected message while awaiting tool approval: {other:?}"),
        }
    };

    // Control requests are rejected with BUSY, not dropped and not
    // method_not_found.
    let err = sender
        .request::<_, ModelListResult>(method::MODEL_LIST, &serde_json::json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.code, crate::jsonrpc::RpcError::BUSY);
    assert!(
        err.message.contains("turn in progress"),
        "unexpected busy error: {}",
        err.message
    );

    // Cancel still works mid-turn; reject the pending approval so the
    // blocked turn unwinds deterministically.
    sender
        .notify(method::CANCEL, &serde_json::Value::Null)
        .await
        .unwrap();
    sender
        .respond_ok(approval_id, ToolApprovalDto::Rejected)
        .await
        .unwrap();

    let result = prompt_task.await.unwrap().unwrap();
    assert!(!result.turn_id.is_empty());
    while client.try_recv_incoming().is_some() {}

    // Between turns the same control request succeeds again.
    let listed: ModelListResult = sender
        .request(method::MODEL_LIST, &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(listed.current, swink_agent::testing::default_model());

    sender
        .notify(method::SHUTDOWN, &serde_json::Value::Null)
        .await
        .unwrap();
    server_task.await.unwrap();
}

fn collect_agent_event(incoming: IncomingMessage, events: &mut Vec<AgentEvent>) {
    let IncomingMessage::Notification { method: m, params } = incoming else {
        panic!("unexpected request while collecting prompt events");
    };
    assert_eq!(m, method::AGENT_EVENT);
    let event = serde_json::from_value(params.expect("agent.event should carry params"))
        .expect("agent.event should deserialize");
    events.push(event);
}

async fn handle_prompt_incoming(
    incoming: IncomingMessage,
    sender: &crate::jsonrpc::PeerSender,
    events: &mut Vec<AgentEvent>,
    approvals: &mut usize,
) {
    match incoming {
        IncomingMessage::Notification { method: m, params } => {
            assert_eq!(m, method::AGENT_EVENT);
            let event = serde_json::from_value(params.expect("agent.event should carry params"))
                .expect("agent.event should deserialize");
            events.push(event);
        }
        IncomingMessage::Request {
            id,
            method: m,
            params,
        } => {
            assert_eq!(m, method::TOOL_APPROVE);
            let request: ToolApprovalRequestDto =
                serde_json::from_value(params.expect("tool.approve should carry params"))
                    .expect("tool.approve params should deserialize");
            assert_eq!(request.id, "call-1");
            assert_eq!(request.name, "dangerous_tool");
            assert_eq!(request.arguments["path"], "/tmp/example");
            assert!(request.requires_approval);

            *approvals += 1;
            sender
                .respond_ok(id, ToolApprovalDto::Approved)
                .await
                .unwrap();
        }
    }
}

#[cfg(not(unix))]
#[tokio::test]
async fn serve_reports_unix_transport_unavailable_on_non_unix_hosts() {
    let server = AgentServer::bind_force("unused.sock", || Ok(test_agent_options("unused")));

    let err = server.serve().await.unwrap_err();

    assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
    assert!(
        err.to_string().contains("Unix socket transport"),
        "unexpected error message: {err}"
    );
}
