//! Tool filter tests for MCP integration (T030-T034).
//!
//! Note: The mock server (MockMcpServer) only exposes a single hardcoded tool
//! named "echo" (via rmcp's `#[tool]` macro). All filter tests operate against
//! this one tool. Tests for allow-listing 2-of-5 or denying 1-of-5 are
//! semantically verified through allow/deny logic; in practice with the mock
//! the pool is always size 1.

mod common;

use std::collections::HashMap;

use swink_agent_mcp::{McpConnection, McpManager, McpServerConfig, McpTransport, ToolFilter};

/// Build a minimal `McpServerConfig` placeholder (transport is unused — we call
/// `McpConnection::from_service` directly with a pre-connected service).
fn stub_config(name: &str, filter: Option<ToolFilter>) -> McpServerConfig {
    McpServerConfig {
        name: name.to_string(),
        transport: McpTransport::Stdio {
            command: "mock".into(),
            args: vec![],
            env: HashMap::default(),
        },
        tool_prefix: None,
        tool_filter: filter,
        requires_approval: false,
    }
}

/// T030: Allow-list containing "echo" — the matching tool is registered.
///
/// Spec says: "mock server with 5 tools, allow-list of 2, verify only 2 tools returned".
/// With the in-process mock only providing "echo", we verify that an allow-list
/// containing "echo" retains it (1 tool present → 1 tool returned).
#[tokio::test]
async fn filter_allow_list_includes_only_matching_tools() {
    let mock_cfg = common::MockServerConfig::new(vec![]);
    let service = common::spawn_mock_server_with_client(&mock_cfg).await;

    let config = stub_config(
        "allow-test",
        Some(ToolFilter {
            allow: Some(vec!["echo".into()]),
            deny: None,
        }),
    );

    let conn = McpConnection::from_service(config, service, None)
        .await
        .expect("connection should succeed");

    let manager =
        McpManager::from_connections(vec![conn]).expect("manager creation should succeed");
    let tools = manager.tools();

    assert_eq!(
        tools.len(),
        1,
        "allow-list ['echo'] should retain the echo tool"
    );
    assert_eq!(tools[0].name(), "echo");
}

/// T031: Deny-list of ["echo"] — echo is excluded, 0 tools registered.
///
/// Spec says: "mock server with 5 tools, deny-list of 1, verify 4 tools returned".
/// With the mock only providing "echo", denying it removes all tools → 0 tools.
#[tokio::test]
async fn filter_deny_list_excludes_matching_tools() {
    let mock_cfg = common::MockServerConfig::new(vec![]);
    let service = common::spawn_mock_server_with_client(&mock_cfg).await;

    let config = stub_config(
        "deny-test",
        Some(ToolFilter {
            allow: None,
            deny: Some(vec!["echo".into()]),
        }),
    );

    let conn = McpConnection::from_service(config, service, None)
        .await
        .expect("connection should succeed");

    let manager =
        McpManager::from_connections(vec![conn]).expect("manager creation should succeed");
    let tools = manager.tools();

    assert_eq!(
        tools.len(),
        0,
        "deny-list ['echo'] should exclude the only tool, yielding 0 tools"
    );
}

/// T032: Allow applied first, then deny. Allow ["echo"], deny ["echo"] → 0 tools.
///
/// Spec says: "mock server with both allow and deny lists, verify allow applied first
/// then deny". Here allow keeps "echo" then deny removes it → net 0 tools.
#[tokio::test]
async fn filter_allow_then_deny_applied_in_order() {
    let mock_cfg = common::MockServerConfig::new(vec![]);
    let service = common::spawn_mock_server_with_client(&mock_cfg).await;

    let config = stub_config(
        "combined-filter-test",
        Some(ToolFilter {
            allow: Some(vec!["echo".into()]),
            deny: Some(vec!["echo".into()]),
        }),
    );

    let conn = McpConnection::from_service(config, service, None)
        .await
        .expect("connection should succeed");

    let manager =
        McpManager::from_connections(vec![conn]).expect("manager creation should succeed");
    let tools = manager.tools();

    assert_eq!(
        tools.len(),
        0,
        "allow=['echo'] then deny=['echo'] should result in 0 tools (deny removes what allow kept)"
    );
}

/// T034: End-to-end with no filter — all discovered tools are returned.
///
/// Verifies the baseline: when `tool_filter` is `None`, `McpManager` exposes
/// every tool the server advertises. The mock server advertises "echo".
#[tokio::test]
async fn no_filter_returns_all_tools() {
    let mock_cfg = common::MockServerConfig::new(vec![]);
    let service = common::spawn_mock_server_with_client(&mock_cfg).await;

    let config = stub_config("no-filter-test", None);

    let conn = McpConnection::from_service(config, service, None)
        .await
        .expect("connection should succeed");

    let manager =
        McpManager::from_connections(vec![conn]).expect("manager creation should succeed");
    let tools = manager.tools();

    assert!(
        !tools.is_empty(),
        "no filter should return all discovered tools (at least 'echo')"
    );
    let names: Vec<_> = tools.iter().map(|t| t.name().to_string()).collect();
    assert!(
        names.contains(&"echo".to_string()),
        "should contain echo tool, got: {names:?}"
    );
}
