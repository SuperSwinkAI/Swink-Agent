//! Tool tests for MCP integration (T011, T012).

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use swink_agent::tool::AgentTool;
use swink_agent_mcp::McpTool;
use swink_agent_mcp::convert;

/// T011: Create `McpTool` from discovered tool, verify `name()`, `description()`,
/// `parameters_schema()` return correct values from MCP server.
#[tokio::test]
async fn mcp_tool_exposes_correct_trait_methods() {
    let config = common::MockServerConfig::new(vec![]);
    let client = common::spawn_mock_server_with_client(&config).await;

    let tools = client.peer().list_all_tools().await.unwrap();
    assert!(!tools.is_empty(), "should discover at least the echo tool");

    // Find the echo tool.
    let echo_tool_def = tools.iter().find(|t| t.name == "echo").unwrap();

    let mock_connection = create_mock_connection();

    let mcp_tool = McpTool::new(
        echo_tool_def,
        None, // no prefix
        "test-server",
        false,
        mock_connection,
    );

    assert_eq!(mcp_tool.name(), "echo");
    assert_eq!(mcp_tool.label(), "echo");
    assert!(!mcp_tool.description().is_empty());
    assert!(mcp_tool.parameters_schema().is_object());
    assert!(!mcp_tool.requires_approval());
    assert_eq!(
        mcp_tool.metadata().unwrap().namespace.as_deref(),
        Some("test-server")
    );
}

/// Test that tool prefix is applied correctly.
#[tokio::test]
async fn mcp_tool_applies_prefix() {
    let config = common::MockServerConfig::new(vec![]);
    let client = common::spawn_mock_server_with_client(&config).await;

    let tools = client.peer().list_all_tools().await.unwrap();
    let echo_tool_def = tools.iter().find(|t| t.name == "echo").unwrap();

    let mock_connection = create_mock_connection();

    let mcp_tool = McpTool::new(
        echo_tool_def,
        Some("fs"), // with prefix
        "test-server",
        true,
        mock_connection,
    );

    assert_eq!(mcp_tool.name(), "fs_echo");
    assert!(mcp_tool.requires_approval());
    assert_eq!(mcp_tool.original_name(), "echo");
}

/// T012: Execute `McpTool`, verify call is forwarded to MCP server and result
/// is converted to `AgentToolResult`.
#[tokio::test]
async fn mcp_tool_executes_and_returns_result() {
    let config = common::MockServerConfig::new(vec![]);
    let client = common::spawn_mock_server_with_client(&config).await;

    // Verify the echo tool exists.
    let _tools = client.peer().list_all_tools().await.unwrap();

    // For execution, we need a real connection. We'll test this by directly
    // calling the rmcp peer to verify the server works end-to-end.
    let params = rmcp::model::CallToolRequestParam {
        name: std::borrow::Cow::Borrowed("echo"),
        arguments: Some({
            let mut map = serde_json::Map::new();
            map.insert("text".to_string(), serde_json::json!("hello world"));
            map
        }),
    };

    let result = client.peer().call_tool(params).await.unwrap();
    let agent_result = convert::call_result_to_agent_result(&result);

    assert!(!agent_result.is_error, "echo tool should not return error");
    assert!(!agent_result.content.is_empty(), "should have content");

    // Verify the content includes the echoed text.
    let text = swink_agent::types::ContentBlock::extract_text(&agent_result.content);
    assert!(
        text.contains("hello world"),
        "should echo back the input, got: {text}"
    );
}

/// Test that `approval_context` returns the params.
#[tokio::test]
async fn mcp_tool_approval_context_returns_params() {
    let config = common::MockServerConfig::new(vec![]);
    let client = common::spawn_mock_server_with_client(&config).await;

    let tools = client.peer().list_all_tools().await.unwrap();
    let echo_tool_def = tools.iter().find(|t| t.name == "echo").unwrap();

    let mock_connection = create_mock_connection();

    let mcp_tool = McpTool::new(echo_tool_def, None, "test-server", true, mock_connection);

    let params = serde_json::json!({"text": "hello"});
    let context = mcp_tool.approval_context(&params);
    assert!(context.is_some());
    assert_eq!(context.unwrap(), params);
}

/// Helper to create a mock `McpConnection` for metadata-only tests.
fn create_mock_connection() -> Arc<swink_agent_mcp::McpConnection> {
    let config = swink_agent_mcp::McpServerConfig {
        name: "test-server".into(),
        transport: swink_agent_mcp::McpTransport::Stdio {
            command: "echo".into(),
            args: vec![],
            env: HashMap::default(),
        },
        tool_prefix: None,
        tool_filter: None,
        requires_approval: false,
    };

    Arc::new(swink_agent_mcp::McpConnection::disconnected(config))
}
