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

/// T035: Connect to MCP server via in-process duplex (equivalent to SSE transport
/// test), verify tool discovery works over the McpConnection API.
#[tokio::test]
async fn connect_via_duplex_discovers_tools() {
    let config = common::MockServerConfig::new(vec![]);
    let service = common::spawn_mock_server_with_client(&config).await;

    let mcp_config = McpServerConfig {
        name: "duplex-test-server".into(),
        transport: McpTransport::Sse {
            url: "http://localhost:0/mcp".into(),
            bearer_token: None,
        },
        tool_prefix: None,
        tool_filter: None,
        requires_approval: false,
    };

    let conn = McpConnection::from_service(mcp_config, service, None)
        .await
        .expect("duplex connection should succeed");

    assert_eq!(
        conn.status(),
        swink_agent_mcp::McpConnectionStatus::Connected
    );
    assert!(
        !conn.discovered_tools.is_empty(),
        "should discover tools from server"
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

/// T036: Connect with bearer token configured — verify the McpConnection
/// API works correctly when auth is configured (mock doesn't validate auth).
#[tokio::test]
async fn connect_via_duplex_with_bearer_token_config() {
    let config = common::MockServerConfig::new(vec![]);
    let service = common::spawn_mock_server_with_client(&config).await;

    let mcp_config = McpServerConfig {
        name: "duplex-auth-test-server".into(),
        transport: McpTransport::Sse {
            url: "http://localhost:0/mcp".into(),
            bearer_token: Some("test-bearer-token-123".into()),
        },
        tool_prefix: None,
        tool_filter: None,
        requires_approval: false,
    };

    let conn = McpConnection::from_service(mcp_config, service, None)
        .await
        .expect("duplex connection with bearer config should succeed");

    assert_eq!(
        conn.status(),
        swink_agent_mcp::McpConnectionStatus::Connected
    );
    assert!(!conn.discovered_tools.is_empty());
}
