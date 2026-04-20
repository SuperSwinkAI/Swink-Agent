//! Manager tests for MCP integration (T019, T020, T021).

mod common;

use rmcp::model::Tool;
use serde_json::json;
use swink_agent_mcp::{McpManager, McpServerConfig, McpTransport};

/// T019: Connect to two mock servers with prefixes, verify tools are prefixed
/// correctly (prefix_toolname).
#[tokio::test]
async fn two_servers_with_prefixes_produce_prefixed_tools() {
    let conn_a = common::spawn_mock_connection("server-a", Some("db"), vec![]).await;
    let conn_b = common::spawn_mock_connection("server-b", Some("fs"), vec![]).await;

    let manager =
        McpManager::from_connections(vec![conn_a, conn_b]).expect("no collision with prefixes");

    let tools = manager.tools();
    let names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();

    // Both mock servers expose the "echo" tool via the #[tool] macro.
    assert!(
        names.contains(&"db_echo".to_string()),
        "should have db_echo, got: {names:?}"
    );
    assert!(
        names.contains(&"fs_echo".to_string()),
        "should have fs_echo, got: {names:?}"
    );
    assert_eq!(
        names.len(),
        2,
        "should have exactly 2 tools, got: {names:?}"
    );
}

/// T020: Connect to three servers where one fails, verify other two servers'
/// tools are available.
#[tokio::test]
async fn partial_failure_still_discovers_tools_from_healthy_servers() {
    let configs = vec![
        // This server will fail — nonexistent command.
        McpServerConfig {
            name: "broken-server".into(),
            transport: McpTransport::Stdio {
                command: "/tmp/definitely-not-a-real-mcp-server-xyz-12345".into(),
                args: vec![],
                env: Default::default(),
            },
            tool_prefix: Some("broken".into()),
            tool_filter: None,
            requires_approval: false,
        },
    ];

    let mut manager = McpManager::new(configs);
    // connect_all should succeed even though the server fails to connect.
    let result = manager.connect_all().await;
    assert!(
        result.is_ok(),
        "connect_all should not fail on partial errors"
    );

    // No tools from the broken server.
    let tools = manager.tools();
    assert!(
        tools.is_empty(),
        "broken server should not contribute tools"
    );

    // Now test with a mix: two healthy mock connections + the failed one.
    let conn_a = common::spawn_mock_connection("healthy-a", Some("a"), vec![]).await;
    let conn_b = common::spawn_mock_connection("healthy-b", Some("b"), vec![]).await;

    let manager = McpManager::from_connections(vec![conn_a, conn_b])
        .expect("no collision with different prefixes");

    let tools = manager.tools();
    let names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();
    assert_eq!(
        names.len(),
        2,
        "should have tools from both healthy servers"
    );
    assert!(names.contains(&"a_echo".to_string()));
    assert!(names.contains(&"b_echo".to_string()));
}

/// T021: Connect two servers without prefixes that have the same tool name,
/// verify `McpError::ToolNameCollision`.
#[tokio::test]
async fn same_tool_name_without_prefix_causes_collision() {
    let conn_a = common::spawn_mock_connection("server-a", None, vec![]).await;
    let conn_b = common::spawn_mock_connection("server-b", None, vec![]).await;

    // Both expose "echo" without prefix — collision.
    let result = McpManager::from_connections(vec![conn_a, conn_b]);
    assert!(result.is_err(), "should detect tool name collision");

    let err = result.unwrap_err();
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("echo"),
        "error should mention the colliding tool name, got: {err_msg}"
    );
    assert!(
        err_msg.contains("server-a") || err_msg.contains("server-b"),
        "error should mention at least one server name, got: {err_msg}"
    );
}

#[test]
fn sanitized_tool_name_collision_is_detected() {
    let mut conn_a = swink_agent_mcp::McpConnection::disconnected(mock_config("server-a"));
    conn_a.discovered_tools = vec![mock_tool("read.file")];

    let mut conn_b = swink_agent_mcp::McpConnection::disconnected(mock_config("server-b"));
    conn_b.discovered_tools = vec![mock_tool("read-file")];

    let err = McpManager::from_connections(vec![conn_a, conn_b]).expect_err("collision expected");
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("read_file"),
        "sanitized colliding name should be reported, got: {err_msg}"
    );
}

fn mock_config(server_name: &str) -> McpServerConfig {
    McpServerConfig {
        name: server_name.to_string(),
        transport: McpTransport::Stdio {
            command: "mock".into(),
            args: vec![],
            env: Default::default(),
        },
        tool_prefix: None,
        tool_filter: None,
        requires_approval: false,
    }
}

fn mock_tool(name: &str) -> Tool {
    let schema = json!({
        "type": "object",
        "properties": {},
    });
    Tool::new(
        name.to_owned(),
        format!("Mock tool: {name}"),
        schema.as_object().expect("object schema").clone(),
    )
}
