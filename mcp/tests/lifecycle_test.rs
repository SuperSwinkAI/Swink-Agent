//! Lifecycle management tests for MCP integration (T039, T040, T043).

mod common;

use std::collections::HashMap;
use std::sync::Arc;

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
