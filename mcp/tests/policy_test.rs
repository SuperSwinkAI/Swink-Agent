//! Policy and approval tests for MCP tools (T026-T029).

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use swink_agent::AgentTool;
use swink_agent_mcp::{McpConnection, McpServerConfig, McpTool, McpTransport};

/// Helper to create a disconnected McpConnection for metadata-only tests.
fn disconnected_connection(requires_approval: bool) -> (McpServerConfig, Arc<McpConnection>) {
    let config = McpServerConfig {
        name: "policy-test-server".into(),
        transport: McpTransport::Stdio {
            command: "mock".into(),
            args: vec![],
            env: HashMap::default(),
        },
        tool_prefix: None,
        tool_filter: None,
        requires_approval,
    };
    let conn = Arc::new(McpConnection::disconnected(config.clone()));
    (config, conn)
}

/// T027: McpTool with requires_approval=true returns true from trait method.
#[tokio::test]
async fn mcp_tool_requires_approval_true_when_configured() {
    let config = common::MockServerConfig::new(vec![]);
    let client = common::spawn_mock_server_with_client(&config).await;
    let tools = client.peer().list_all_tools().await.unwrap();
    let echo_def = tools.iter().find(|t| t.name == "echo").unwrap();

    let (_, conn) = disconnected_connection(true);
    let tool = McpTool::new(echo_def, None, "policy-test-server", true, conn);

    assert!(
        tool.requires_approval(),
        "requires_approval should be true when configured as true"
    );
}

/// T028: McpTool with requires_approval=false returns false.
#[tokio::test]
async fn mcp_tool_requires_approval_false_when_configured() {
    let config = common::MockServerConfig::new(vec![]);
    let client = common::spawn_mock_server_with_client(&config).await;
    let tools = client.peer().list_all_tools().await.unwrap();
    let echo_def = tools.iter().find(|t| t.name == "echo").unwrap();

    let (_, conn) = disconnected_connection(false);
    let tool = McpTool::new(echo_def, None, "policy-test-server", false, conn);

    assert!(
        !tool.requires_approval(),
        "requires_approval should be false when configured as false"
    );
}

/// T029: approval_context returns the full params as context.
#[tokio::test]
async fn mcp_tool_approval_context_returns_params_for_policy_inspection() {
    let config = common::MockServerConfig::new(vec![]);
    let client = common::spawn_mock_server_with_client(&config).await;
    let tools = client.peer().list_all_tools().await.unwrap();
    let echo_def = tools.iter().find(|t| t.name == "echo").unwrap();

    let (_, conn) = disconnected_connection(true);
    let tool = McpTool::new(echo_def, None, "policy-test-server", true, conn);

    let params = serde_json::json!({
        "text": "sensitive-input",
        "path": "/etc/passwd"
    });
    let context = tool.approval_context(&params);

    assert!(
        context.is_some(),
        "approval_context should return Some for MCP tools"
    );
    assert_eq!(
        context.unwrap(),
        params,
        "approval context should be the full params so policies can inspect arguments"
    );
}

/// T026: approval_context is non-None — policies receive params for inspection.
/// Verifies the contract that MCP tools always expose params to approval/policy gates.
#[tokio::test]
async fn mcp_tool_always_provides_approval_context() {
    let config = common::MockServerConfig::new(vec![]);
    let client = common::spawn_mock_server_with_client(&config).await;
    let tools = client.peer().list_all_tools().await.unwrap();
    let echo_def = tools.iter().find(|t| t.name == "echo").unwrap();

    let (_, conn) = disconnected_connection(true);
    let tool = McpTool::new(echo_def, None, "policy-test-server", true, conn);

    // Empty params
    let empty = Value::Null;
    assert!(
        tool.approval_context(&empty).is_some(),
        "approval_context must be Some even for null params — policies must always be able to inspect MCP tool calls"
    );

    // Object params
    let obj = serde_json::json!({"key": "value"});
    assert!(
        tool.approval_context(&obj).is_some(),
        "approval_context must be Some for object params"
    );
}
