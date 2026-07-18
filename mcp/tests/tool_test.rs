//! Tool tests for MCP integration (T011, T012).

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;
use swink_agent::AgentTool;
use swink_agent::{MAX_TOOL_NAME_LEN, TOOL_NAME_HASH_HEX_LEN};
use swink_agent_mcp::{McpTool, McpToolInfo};

/// T011: Create `McpTool` from discovered tool, verify `name()`, `description()`,
/// `parameters_schema()` return correct values from MCP server.
#[tokio::test]
async fn mcp_tool_exposes_correct_trait_methods() {
    let echo_tool_def = common::echo_tool_info().await;

    let mock_connection = create_mock_connection();

    let mcp_tool = McpTool::new(
        &echo_tool_def,
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
    let echo_tool_def = common::echo_tool_info().await;

    let mock_connection = create_mock_connection();

    let mcp_tool = McpTool::new(
        &echo_tool_def,
        Some("fs"), // with prefix
        "test-server",
        true,
        mock_connection,
    );

    assert_eq!(mcp_tool.name(), "fs_echo");
    assert!(mcp_tool.requires_approval());
    assert_eq!(mcp_tool.original_name(), "echo");
}

#[tokio::test]
async fn mcp_tool_sanitizes_unsafe_name_without_prefix() {
    let mock_connection = create_mock_connection();
    let tool = mock_tool("read.file");

    let mcp_tool = McpTool::new(&tool, None, "test-server", false, mock_connection);

    assert_eq!(mcp_tool.name(), "read_file");
    assert_eq!(mcp_tool.original_name(), "read.file");
}

#[tokio::test]
async fn mcp_tool_truncates_long_prefixed_name_with_hash_suffix() {
    let mock_connection = create_mock_connection();
    let tool = mock_tool(&"b".repeat(80));

    let mcp_tool = McpTool::new(
        &tool,
        Some(&"a".repeat(40)),
        "test-server",
        false,
        mock_connection,
    );

    assert_eq!(mcp_tool.name().len(), MAX_TOOL_NAME_LEN);
    assert_eq!(
        mcp_tool
            .name()
            .rsplit_once('_')
            .expect("hash suffix")
            .1
            .len(),
        TOOL_NAME_HASH_HEX_LEN
    );
    assert_eq!(mcp_tool.original_name(), "b".repeat(80));
}

/// T012: Execute a tool call end-to-end, verify the call is forwarded to the
/// MCP server and the result is converted to `AgentToolResult`.
#[tokio::test]
async fn mcp_tool_executes_and_returns_result() {
    let conn = common::spawn_mock_connection("exec-test-server", None, vec![]).await;

    let agent_result = conn
        .call_tool("echo", serde_json::json!({"text": "hello world"}))
        .await
        .expect("echo call should succeed");

    assert!(!agent_result.is_error, "echo tool should not return error");
    assert!(!agent_result.content.is_empty(), "should have content");

    // Verify the content includes the echoed text.
    let text = swink_agent::ContentBlock::extract_text(&agent_result.content);
    assert!(
        text.contains("hello world"),
        "should echo back the input, got: {text}"
    );
}

/// Test that `approval_context` returns the params.
#[tokio::test]
async fn mcp_tool_approval_context_returns_params() {
    let echo_tool_def = common::echo_tool_info().await;

    let mock_connection = create_mock_connection();

    let mcp_tool = McpTool::new(&echo_tool_def, None, "test-server", true, mock_connection);

    let params = serde_json::json!({"text": "hello"});
    let context = mcp_tool.approval_context(&params);
    assert!(context.is_some());
    assert_eq!(context.unwrap(), params);
}

/// Helper to create a mock `McpConnection` for metadata-only tests.
fn create_mock_connection() -> Arc<swink_agent_mcp::McpConnection> {
    let config = swink_agent_mcp::McpServerConfig::new(
        "test-server",
        swink_agent_mcp::McpTransport::Stdio {
            command: "echo".into(),
            args: vec![],
            env: HashMap::default(),
        },
    )
    .with_requires_approval(false);

    Arc::new(swink_agent_mcp::McpConnection::disconnected(config))
}

fn mock_tool(name: &str) -> McpToolInfo {
    McpToolInfo::new(
        name,
        format!("Mock tool: {name}"),
        json!({
            "type": "object",
            "properties": {},
        }),
    )
}
