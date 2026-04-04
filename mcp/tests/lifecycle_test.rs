//! Lifecycle management tests for MCP integration (T039, T040, T042, T043).

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use swink_agent::tool::AgentTool;
use swink_agent_mcp::{McpConnection, McpManager, McpServerConfig, McpTransport, McpTool};

/// T039: Drop McpManager cleans up without hang or panic.
///
/// Uses an in-process mock connection (no real subprocess), so we verify
/// the Drop impl runs without issues. For real subprocess cleanup,
/// rmcp's ChildWithCleanup handles SIGKILL on drop.
#[tokio::test]
async fn drop_manager_cleans_up_without_panic() {
    let conn = common::spawn_mock_connection("lifecycle-test", None, vec![]).await;

    let manager = McpManager::from_connections(vec![conn])
        .expect("manager creation should succeed");

    assert!(!manager.tools().is_empty(), "should have tools before drop");

    // Drop the manager — this exercises the Drop impl.
    drop(manager);
    // If we reach here without hanging, cleanup succeeded.
}

/// T040: call_tool on a disconnected McpConnection returns an error immediately.
///
/// Verifies graceful degradation when MCP server is unavailable.
#[tokio::test]
async fn call_tool_on_disconnected_connection_returns_error() {
    let config = McpServerConfig {
        name: "disconnected-server".into(),
        transport: McpTransport::Stdio {
            command: "mock".into(),
            args: vec![],
            env: HashMap::default(),
        },
        tool_prefix: None,
        tool_filter: None,
        requires_approval: false,
    };
    let conn = McpConnection::disconnected(config);

    let result = conn.call_tool("echo", serde_json::json!({"text": "hello"})).await;

    assert!(result.is_err(), "call_tool on disconnected connection should return Err");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("disconnected") || msg.contains("server is disconnected"),
        "error should mention disconnection, got: {msg}"
    );
}

/// T042: Background monitor detects transport closure, updates status to
/// Disconnected, and emits McpServerDisconnected on the event channel.
///
/// We simulate a crash by properly cancelling the server's `RunningService`,
/// which drops its side of the duplex transport and causes the client to see
/// EOF — triggering `QuitReason::Closed` in the monitor task.
#[tokio::test]
async fn monitor_detects_transport_close_and_emits_event() {
    use rmcp::service::ServiceExt;
    use swink_agent::AgentEvent;
    use swink_agent_mcp::{McpConnection, McpConnectionStatus, McpServerConfig, McpTransport};
    use tokio::sync::mpsc::unbounded_channel;
    use tokio::sync::oneshot;

    let (event_tx, mut event_rx) = unbounded_channel::<AgentEvent>();

    let (client_stream, server_stream) = tokio::io::duplex(4096);

    let mock_cfg = common::MockServerConfig::new(vec![]);
    let server = common::MockMcpServer::from_config(&mock_cfg);

    // Signal channel: test → server task, to cancel the server RunningService.
    // Aborting the outer task only detaches rmcp's internal I/O task (dropping
    // JoinHandle detaches, doesn't abort), so the duplex stays open. We must
    // explicitly cancel the RunningService to drop the transport.
    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        let svc = server.serve(server_stream).await.expect("server should start");
        let _ = cancel_rx.await;
        // cancel() calls ct.cancel() → internal loop breaks → server_stream dropped
        let _ = svc.cancel().await;
    });

    let client_info = rmcp::model::ClientInfo::default();
    let service = client_info
        .serve(client_stream)
        .await
        .expect("client should connect");

    let config = McpServerConfig {
        name: "crash-test-server".into(),
        transport: McpTransport::Stdio {
            command: "mock".into(),
            args: vec![],
            env: HashMap::default(),
        },
        tool_prefix: None,
        tool_filter: None,
        requires_approval: false,
    };

    let conn = McpConnection::from_service(config, service, Some(event_tx))
        .await
        .expect("connection should succeed");

    assert_eq!(conn.status(), McpConnectionStatus::Connected, "should start Connected");

    // Simulate crash: cancel the server → drops server_stream → client sees EOF.
    let _ = cancel_tx.send(());
    let _ = server_task.await;

    // Poll until the monitor detects QuitReason::Closed and updates status.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if conn.status() == McpConnectionStatus::Disconnected {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "monitor did not detect disconnect within 2 seconds"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    assert_eq!(
        conn.status(),
        McpConnectionStatus::Disconnected,
        "monitor should have transitioned status to Disconnected"
    );

    // The disconnect event should have been emitted.
    let event = event_rx.try_recv().expect("McpServerDisconnected event should be in channel");
    match event {
        AgentEvent::McpServerDisconnected { server_name, .. } => {
            assert_eq!(server_name, "crash-test-server");
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

/// T043: McpTool::execute() on a disconnected connection returns is_error=true.
///
/// Verifies that the execute() path handles disconnected connections by
/// returning an error AgentToolResult, not panicking or hanging.
#[tokio::test]
async fn mcp_tool_execute_on_disconnected_returns_error_result() {
    // Get a real tool definition from the mock server.
    let mock_cfg = common::MockServerConfig::new(vec![]);
    let client = common::spawn_mock_server_with_client(&mock_cfg).await;
    let tools = client.peer().list_all_tools().await.unwrap();
    let echo_def = tools.iter().find(|t| t.name == "echo").unwrap();

    // Create a disconnected connection.
    let config = McpServerConfig {
        name: "disconnected-server".into(),
        transport: McpTransport::Stdio {
            command: "mock".into(),
            args: vec![],
            env: HashMap::default(),
        },
        tool_prefix: None,
        tool_filter: None,
        requires_approval: false,
    };
    let conn = Arc::new(McpConnection::disconnected(config));
    let tool = McpTool::new(echo_def, None, "disconnected-server", false, conn);

    let cancel = tokio_util::sync::CancellationToken::new();
    let state = Arc::new(std::sync::RwLock::new(swink_agent::SessionState::default()));

    let result = tool
        .execute(
            "call-123",
            serde_json::json!({"text": "hello"}),
            cancel,
            None,
            state,
            None,
        )
        .await;

    assert!(
        result.is_error,
        "execute on disconnected McpTool should return is_error=true"
    );
}
