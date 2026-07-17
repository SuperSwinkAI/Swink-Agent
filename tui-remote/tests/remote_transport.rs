//! End-to-end tests: `RemoteTransport` against a real `AgentServer` on a
//! Unix socket, mirroring the harness in `rpc/tests/end_to_end.rs`.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::sync::Arc;
use std::time::Duration;

use swink_agent::{AgentEvent, AgentOptions, ContentBlock, StreamFn};
use swink_agent_rpc::AgentServer;
use swink_agent_tui::{TuiTransport, UserInput};
use swink_agent_tui_remote::RemoteTransport;

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

/// Poll until the socket file exists with the server's 0600 permissions.
async fn wait_for_secured_socket(path: &std::path::Path) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(meta) = std::fs::metadata(path)
                && meta.permissions().mode() & 0o777 == 0o600
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("server did not bind and secure the socket in time");
}

/// Drain events from the transport until `AgentEnd` (inclusive), with a timeout.
async fn collect_turn(transport: &mut RemoteTransport) -> Vec<AgentEvent> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut events = Vec::new();
        while let Some(event) = transport.recv().await {
            let done = matches!(event, AgentEvent::AgentEnd { .. });
            events.push(event);
            if done {
                break;
            }
        }
        events
    })
    .await
    .expect("turn did not complete in time")
}

fn turn_has_text(events: &[AgentEvent], expected: &str) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::MessageEnd { message } if message
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::Text { text } if text == expected))
        )
    })
}

#[tokio::test]
async fn remote_transport_round_trips_a_turn_over_a_real_socket() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("swink.sock");

    let server = AgentServer::bind(&path, || Ok(test_agent_options("hello from agentd"))).unwrap();
    let server_task = tokio::spawn(server.serve());
    wait_for_secured_socket(&path).await;

    let mut transport = RemoteTransport::connect(&path).await.unwrap();

    transport
        .send(UserInput::new("hi"))
        .await
        .expect("send should be accepted");

    let events = collect_turn(&mut transport).await;
    assert!(
        turn_has_text(&events, "hello from agentd"),
        "expected the assistant response in the streamed events, got: {events:?}"
    );

    server_task.abort();
    let _ = server_task.await;
}

#[tokio::test]
async fn remote_transport_survives_multiple_sequential_turns() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("swink.sock");

    let server = AgentServer::bind(&path, || Ok(test_agent_options("again"))).unwrap();
    let server_task = tokio::spawn(server.serve());
    wait_for_secured_socket(&path).await;

    let mut transport = RemoteTransport::connect(&path).await.unwrap();

    for turn in 0..2 {
        transport
            .send(UserInput::new(format!("turn {turn}")))
            .await
            .expect("send should be accepted");
        let events = collect_turn(&mut transport).await;
        assert!(
            turn_has_text(&events, "again"),
            "turn {turn} should stream the assistant response, got: {events:?}"
        );
    }

    server_task.abort();
    let _ = server_task.await;
}

#[tokio::test]
async fn try_recv_returns_none_when_no_events_are_queued() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("swink.sock");

    let server = AgentServer::bind(&path, || Ok(test_agent_options("unused"))).unwrap();
    let server_task = tokio::spawn(server.serve());
    wait_for_secured_socket(&path).await;

    let mut transport = RemoteTransport::connect(&path).await.unwrap();
    assert!(transport.try_recv().is_none(), "no events queued yet");

    server_task.abort();
    let _ = server_task.await;
}

#[tokio::test]
async fn failed_turn_surfaces_a_synthetic_agent_end() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("swink.sock");

    let server = AgentServer::bind(&path, || Ok(test_agent_options("unused"))).unwrap();
    let server_task = tokio::spawn(server.serve());
    wait_for_secured_socket(&path).await;

    let mut transport = RemoteTransport::connect(&path).await.unwrap();

    // Tear the server down mid-session so the next turn fails.
    server_task.abort();
    let _ = server_task.await;

    transport
        .send(UserInput::new("into the void"))
        .await
        .expect("send is accepted by the local channel");

    let events = collect_turn(&mut transport).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::AgentEnd { .. })),
        "a failed turn must still end with AgentEnd so the UI leaves its streaming state"
    );
}
