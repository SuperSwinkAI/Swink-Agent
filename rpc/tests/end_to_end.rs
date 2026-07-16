//! Real Unix-socket integration tests for `AgentServer` / `AgentClient`.
//!
//! Unlike `tests/peer.rs` (which drives the JSON-RPC peer over in-memory
//! `tokio::io::duplex` pairs) and the `server.rs` unit tests (same, at the
//! session level), these tests exercise an actual `UnixListener` bound to a
//! socket path in a tempdir, using the crate's public client/server API.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::sync::Arc;
use std::time::Duration;

use swink_agent::{AgentEvent, AgentOptions, ContentBlock, StreamFn};
use swink_agent_rpc::{AgentClient, AgentServer};

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

/// Poll until the socket file exists and carries the server's 0600
/// permissions, or panic after 2s. `serve()` binds and `chmod`s the socket
/// before it does anything else, but that happens on the spawned task's own
/// schedule, so callers can't assume it's done the instant `spawn` returns.
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

#[tokio::test]
async fn server_accepts_real_socket_connection_and_round_trips_a_prompt() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("swink.sock");

    let server =
        AgentServer::bind(&path, || Ok(test_agent_options("hello over the wire"))).unwrap();
    let server_task = tokio::spawn(server.serve());

    wait_for_secured_socket(&path).await;

    let perms = std::fs::metadata(&path).unwrap().permissions();
    assert_eq!(
        perms.mode() & 0o777,
        0o600,
        "socket file should be owner-only (0600)"
    );

    let mut client = AgentClient::connect(&path).await.unwrap();
    let events = client.prompt_text("hello").await.unwrap();
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::MessageEnd { message } if message
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::Text { text } if text == "hello over the wire"))
        )),
        "client should receive the assistant response over the real socket"
    );

    client.shutdown().await.unwrap();

    // `shutdown` only ends this client's session; the accept loop keeps
    // running for further connections (that's what SIGTERM/Ctrl-C is for).
    // Aborting the task drops the in-flight `serve()` future, which runs the
    // `SocketCleanup` guard's destructor the same way a real signal-driven
    // shutdown would.
    server_task.abort();
    let _ = server_task.await;

    assert!(
        !path.exists(),
        "socket file should be removed once the server task is torn down"
    );
}

#[tokio::test]
async fn second_bind_attempt_without_force_fails_while_server_is_active() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("swink.sock");

    let server = AgentServer::bind(&path, || Ok(test_agent_options("unused"))).unwrap();
    let server_task = tokio::spawn(server.serve());

    wait_for_secured_socket(&path).await;

    let err = match AgentServer::bind(&path, || Ok(test_agent_options("unused"))) {
        Ok(_) => panic!("bind should reject an already-active socket path without --force"),
        Err(err) => err,
    };
    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    assert!(
        err.to_string().contains("remove it or pass --force"),
        "unexpected bind error: {err}"
    );

    server_task.abort();
    let _ = server_task.await;
}
