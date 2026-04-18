//! Lifecycle management tests for MCP integration (T039, T040, T041, T042, T043).

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use swink_agent::{AgentEvent, AgentTool, ContentBlock, SessionState};
use swink_agent_mcp::{
    McpConnection, McpConnectionStatus, McpManager, McpServerConfig, McpTool, McpTransport,
};
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::oneshot;

/// T039: Drop McpManager cleans up without hang or panic.
///
/// Uses an in-process mock connection (no real subprocess), so we verify
/// the Drop impl runs without issues. For real subprocess cleanup,
/// rmcp's ChildWithCleanup handles SIGKILL on drop.
#[tokio::test]
async fn drop_manager_cleans_up_without_panic() {
    let conn = common::spawn_mock_connection("lifecycle-test", None, vec![]).await;

    let manager =
        McpManager::from_connections(vec![conn]).expect("manager creation should succeed");

    assert!(!manager.tools().is_empty(), "should have tools before drop");

    drop(manager);
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

    let result = conn
        .call_tool("echo", serde_json::json!({"text": "hello"}))
        .await;

    assert!(
        result.is_err(),
        "call_tool on disconnected connection should return Err"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("disconnected") || msg.contains("server is disconnected"),
        "error should mention disconnection, got: {msg}"
    );
}

/// T041: `McpManager::shutdown()` disconnects shared connections even when
/// exported tool handles are still cloned by the caller.
#[tokio::test]
async fn shutdown_disconnects_cloned_tool_handles() {
    let conn = common::spawn_mock_connection("shared-shutdown-test", None, vec![]).await;
    let mut manager =
        McpManager::from_connections(vec![conn]).expect("manager creation should succeed");

    let kept_tool = manager
        .tools()
        .into_iter()
        .next()
        .expect("manager should expose at least one tool");

    tokio::time::timeout(Duration::from_secs(2), manager.shutdown())
        .await
        .expect("shutdown should complete even when tool handles are still alive");

    let cancel = tokio_util::sync::CancellationToken::new();
    let state = Arc::new(std::sync::RwLock::new(SessionState::default()));

    let result = kept_tool
        .execute(
            "call-shutdown",
            serde_json::json!({"text": "hello"}),
            cancel,
            None,
            state,
            None,
        )
        .await;

    assert!(
        result.is_error,
        "retained tool handles should fail fast after manager shutdown"
    );

    let text = ContentBlock::extract_text(&result.content);
    assert!(
        text.contains("disconnected") || text.contains("active session"),
        "shutdown should disconnect shared tool handles, got: {text}"
    );
}

/// T042: Background monitor detects transport closure, updates status to
/// Disconnected, and emits McpServerDisconnected on the event channel.
///
/// We simulate a crash by properly cancelling the server's `RunningService`,
/// which drops its side of the duplex transport and causes the client to see
/// EOF, triggering `QuitReason::Closed` in the monitor task.
#[tokio::test]
async fn monitor_detects_transport_close_and_emits_event() {
    use rmcp::service::ServiceExt;

    let (event_tx, mut event_rx) = unbounded_channel::<AgentEvent>();

    let (client_stream, server_stream) = tokio::io::duplex(4096);

    let mock_cfg = common::MockServerConfig::new(vec![]);
    let server = common::MockMcpServer::from_config(&mock_cfg);

    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        let svc = server
            .serve(server_stream)
            .await
            .expect("server should start");
        let _ = cancel_rx.await;
        let _ = svc.cancel().await;
    });

    let service = rmcp::model::ClientInfo::default()
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

    assert_eq!(
        conn.status(),
        McpConnectionStatus::Connected,
        "should start Connected"
    );

    // Drain the connect + discovery events emitted during construction so
    // only the disconnect event remains to be asserted below.
    let _connect = event_rx.try_recv().expect("connect event");
    let _discovery = event_rx.try_recv().expect("discovery event");

    let _ = cancel_tx.send(());
    let _ = server_task.await;

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

    let event = event_rx
        .try_recv()
        .expect("McpServerDisconnected event should be in channel");
    match event {
        AgentEvent::McpServerDisconnected { server_name, .. } => {
            assert_eq!(server_name, "crash-test-server");
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

/// Issue #611: `McpServerConnected` is emitted once the handshake completes.
///
/// Uses `from_service` to take a pre-established rmcp service past the
/// handshake boundary and asserts the connect event is the first lifecycle
/// event observed.
#[tokio::test]
async fn connected_event_emitted_after_handshake() {
    let (event_tx, mut event_rx) = unbounded_channel::<AgentEvent>();

    let mock_cfg = common::MockServerConfig::new(vec![]);
    let service = common::spawn_mock_server_with_client(&mock_cfg).await;

    let config = McpServerConfig {
        name: "connect-event-server".into(),
        transport: McpTransport::Stdio {
            command: "mock".into(),
            args: vec![],
            env: HashMap::default(),
        },
        tool_prefix: None,
        tool_filter: None,
        requires_approval: false,
    };

    let _conn = McpConnection::from_service(config, service, Some(event_tx))
        .await
        .expect("connection should succeed");

    let first = event_rx.try_recv().expect("connect event should be queued");
    match first {
        AgentEvent::McpServerConnected { server_name } => {
            assert_eq!(server_name, "connect-event-server");
        }
        other => panic!("expected McpServerConnected first, got: {other:?}"),
    }
}

/// Issue #611: `McpToolsDiscovered` is emitted after the list_tools round trip
/// and carries the discovered tool count.
#[tokio::test]
async fn tools_discovered_event_emitted_after_discovery() {
    let (event_tx, mut event_rx) = unbounded_channel::<AgentEvent>();

    let mock_cfg = common::MockServerConfig::new(vec![]);
    let service = common::spawn_mock_server_with_client(&mock_cfg).await;

    let config = McpServerConfig {
        name: "discovery-event-server".into(),
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

    // Drain events in order and assert we see connect followed by discovery.
    let connect_evt = event_rx.try_recv().expect("connect event should fire");
    assert!(
        matches!(connect_evt, AgentEvent::McpServerConnected { .. }),
        "first event should be McpServerConnected, got: {connect_evt:?}"
    );

    let discovery_evt = event_rx.try_recv().expect("discovery event should fire");
    match discovery_evt {
        AgentEvent::McpToolsDiscovered {
            server_name,
            tool_count,
        } => {
            assert_eq!(server_name, "discovery-event-server");
            assert_eq!(
                tool_count,
                conn.discovered_tools.len(),
                "discovery event tool_count should match connection's discovered tool list"
            );
        }
        other => panic!("expected McpToolsDiscovered after connect, got: {other:?}"),
    }
}

/// Issue #611: Forwarded tool calls are bracketed by
/// `McpToolCallStarted` / `McpToolCallCompleted`, with `is_error=false` for
/// successful calls.
#[tokio::test]
async fn tool_call_events_bracket_successful_call() {
    use swink_agent::AgentTool;

    let (event_tx, mut event_rx) = unbounded_channel::<AgentEvent>();

    let mock_cfg = common::MockServerConfig::new(vec![]);
    let service = common::spawn_mock_server_with_client(&mock_cfg).await;

    let config = McpServerConfig {
        name: "call-event-server".into(),
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

    // Drain the connect + discovery events emitted during construction.
    let _connect = event_rx.try_recv().expect("connect event");
    let _discovery = event_rx.try_recv().expect("discovery event");

    let echo_def = conn
        .discovered_tools
        .iter()
        .find(|t| t.name == "echo")
        .expect("mock server advertises echo tool")
        .clone();
    let conn = Arc::new(conn);
    let tool = McpTool::new(
        &echo_def,
        None,
        "call-event-server",
        false,
        Arc::clone(&conn),
    );

    let cancel = tokio_util::sync::CancellationToken::new();
    let state = Arc::new(std::sync::RwLock::new(SessionState::default()));
    let result = tool
        .execute(
            "call-1",
            serde_json::json!({"text": "hello"}),
            cancel,
            None,
            state,
            None,
        )
        .await;

    assert!(!result.is_error, "echo call should succeed");

    let started = event_rx.try_recv().expect("tool_call_started event");
    match started {
        AgentEvent::McpToolCallStarted {
            server_name,
            tool_name,
        } => {
            assert_eq!(server_name, "call-event-server");
            assert_eq!(tool_name, "echo");
        }
        other => panic!("expected McpToolCallStarted first, got: {other:?}"),
    }

    let completed = event_rx.try_recv().expect("tool_call_completed event");
    match completed {
        AgentEvent::McpToolCallCompleted {
            server_name,
            tool_name,
            is_error,
        } => {
            assert_eq!(server_name, "call-event-server");
            assert_eq!(tool_name, "echo");
            assert!(
                !is_error,
                "successful echo call should report is_error=false"
            );
        }
        other => panic!("expected McpToolCallCompleted, got: {other:?}"),
    }
}

/// Issue #611: A failing tool call still emits `McpToolCallCompleted` with
/// `is_error=true` — the completion event is guaranteed even in the error
/// path so observers never see an unclosed bracket.
#[tokio::test]
async fn tool_call_completed_event_reports_error_on_failure() {
    use swink_agent::AgentTool;

    let (event_tx, mut event_rx) = unbounded_channel::<AgentEvent>();

    let mock_cfg = common::MockServerConfig::new(vec![]);
    let service = common::spawn_mock_server_with_client(&mock_cfg).await;

    let config = McpServerConfig {
        name: "error-call-event-server".into(),
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

    // Drain connect + discovery events so only call-related events remain.
    let _connect = event_rx.try_recv().expect("connect event");
    let _discovery = event_rx.try_recv().expect("discovery event");

    let echo_def = conn
        .discovered_tools
        .iter()
        .find(|t| t.name == "echo")
        .expect("echo tool")
        .clone();

    // Force the call to fail by shutting down the connection before invoking.
    conn.shutdown().await;

    let conn = Arc::new(conn);
    let tool = McpTool::new(
        &echo_def,
        None,
        "error-call-event-server",
        false,
        Arc::clone(&conn),
    );

    let cancel = tokio_util::sync::CancellationToken::new();
    let state = Arc::new(std::sync::RwLock::new(SessionState::default()));
    let result = tool
        .execute(
            "call-err",
            serde_json::json!({"text": "hello"}),
            cancel,
            None,
            state,
            None,
        )
        .await;

    assert!(result.is_error, "disconnected call should error");

    let started = event_rx.try_recv().expect("tool_call_started event");
    assert!(
        matches!(started, AgentEvent::McpToolCallStarted { .. }),
        "first event should be McpToolCallStarted, got: {started:?}"
    );
    let completed = event_rx.try_recv().expect("tool_call_completed event");
    match completed {
        AgentEvent::McpToolCallCompleted { is_error, .. } => {
            assert!(is_error, "failing call should report is_error=true");
        }
        other => panic!("expected McpToolCallCompleted, got: {other:?}"),
    }
}

/// T043: McpTool::execute() on a disconnected connection returns is_error=true.
///
/// Verifies that the execute() path handles disconnected connections by
/// returning an error AgentToolResult, not panicking or hanging.
#[tokio::test]
async fn mcp_tool_execute_on_disconnected_returns_error_result() {
    let mock_cfg = common::MockServerConfig::new(vec![]);
    let client = common::spawn_mock_server_with_client(&mock_cfg).await;
    let tools = client.peer().list_all_tools().await.unwrap();
    let echo_def = tools.iter().find(|t| t.name == "echo").unwrap();

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
    let state = Arc::new(std::sync::RwLock::new(SessionState::default()));

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
