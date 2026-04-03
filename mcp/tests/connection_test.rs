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

    let result = McpConnection::connect(config).await;
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

/// Verify SSE transport returns a clear not-yet-implemented error.
#[tokio::test]
async fn connect_sse_returns_not_implemented() {
    let config = McpServerConfig {
        name: "remote-server".into(),
        transport: McpTransport::Sse {
            url: "http://localhost:9999/mcp".into(),
            bearer_token: None,
        },
        tool_prefix: None,
        tool_filter: None,
        requires_approval: false,
    };

    let result = McpConnection::connect(config).await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("SSE transport not yet implemented"),
        "should indicate SSE is not yet supported, got: {err_msg}"
    );
}
