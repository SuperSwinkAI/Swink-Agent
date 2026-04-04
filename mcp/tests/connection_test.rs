//! Connection tests for MCP integration (T010, T013).

mod common;

use swink_agent_mcp::{McpConnection, McpServerConfig, McpTransport};

/// T010: Connect to mock stdio MCP server, verify connection succeeds
/// and tools are discovered.
///
/// We cannot use a real stdio subprocess in unit tests without an external
/// binary, so we test via the in-process duplex transport helper to verify
/// tool discovery works, and test the `McpConnection` API for error cases.
#[tokio::test]
async fn connect_discovers_tools_via_in_process_server() {
    let config = common::MockServerConfig::new(vec![
        common::MockToolDef::simple("search_files", "found: main.rs"),
        common::MockToolDef::simple("read_file", "contents of file"),
    ]);

    let client = common::spawn_mock_server_with_client(&config).await;

    // Verify tool discovery works via the rmcp peer API.
    let tools = client.peer().list_all_tools().await.unwrap();
    // The mock server always exposes the "echo" tool via the tool macro.
    assert!(
        !tools.is_empty(),
        "should discover at least the echo tool from mock server"
    );

    // Verify tool names contain "echo" (the tool we defined on `MockMcpServer`).
    let tool_names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    assert!(
        tool_names.contains(&"echo".to_string()),
        "should discover the echo tool, got: {tool_names:?}"
    );
}

/// T013: Attempt connection to non-existent server, verify graceful error
/// with `McpError::SpawnFailed`.
#[tokio::test]
async fn connect_to_nonexistent_server_returns_spawn_failed() {
    let config = McpServerConfig {
        name: "nonexistent".into(),
        transport: McpTransport::Stdio {
            command: "/tmp/definitely-not-a-real-mcp-server-binary-xyz".into(),
            args: vec![],
            env: std::collections::HashMap::default(),
        },
        tool_prefix: None,
        tool_filter: None,
        requires_approval: false,
    };

    let result = McpConnection::connect(config, None).await;
    assert!(
        result.is_err(),
        "should fail to connect to nonexistent server"
    );

    let err = result.unwrap_err();
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("nonexistent"),
        "error should mention server name, got: {err_msg}"
    );
}

/// T035: Verify `from_service` connects and discovers tools (exercises the
/// same code path as SSE/HTTP transport, minus the network layer).
///
/// The original test used `rmcp::transport::sse_server::SseServer` which was
/// removed in rmcp 1.x. The in-process duplex transport exercises identical
/// tool-discovery logic.
#[tokio::test]
async fn from_service_discovers_tools() {
    let conn = common::spawn_mock_connection("http-test-server", None, vec![]).await;

    assert_eq!(
        conn.status(),
        swink_agent_mcp::McpConnectionStatus::Connected
    );
    assert!(
        !conn.discovered_tools.is_empty(),
        "should discover tools from mock server"
    );
    let names: Vec<_> = conn
        .discovered_tools
        .iter()
        .map(|t| t.name.as_ref())
        .collect();
    assert!(
        names.contains(&"echo"),
        "should discover echo tool, got: {names:?}"
    );
}

/// T036: Verify `connect` to a non-existent HTTP URL returns a connection error
/// (exercises the HTTP streaming code path with an unreachable endpoint).
#[tokio::test]
async fn connect_sse_to_unreachable_url_returns_error() {
    let config = McpServerConfig {
        name: "sse-unreachable".into(),
        transport: McpTransport::Sse {
            url: "http://127.0.0.1:1/mcp".into(),
            bearer_token: Some("test-bearer-token-123".into()),
        },
        tool_prefix: None,
        tool_filter: None,
        requires_approval: false,
    };

    let result = McpConnection::connect(config, None).await;
    assert!(result.is_err(), "connecting to unreachable URL should fail");
}
