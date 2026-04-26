//! Tool filter tests for MCP integration (T030-T034).
//!
mod common;

use std::collections::HashMap;

use swink_agent_mcp::{McpConnection, McpManager, McpServerConfig, McpTransport, ToolFilter};

fn five_tool_config() -> common::MockServerConfig {
    common::MockServerConfig::new(vec![
        common::MockToolDef::simple("echo", "echo"),
        common::MockToolDef::simple("tool_a", "tool_a"),
        common::MockToolDef::simple("tool_b", "tool_b"),
        common::MockToolDef::simple("tool_c", "tool_c"),
        common::MockToolDef::simple("tool_d", "tool_d"),
    ])
}

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
        connect_timeout_ms: None,
        discovery_timeout_ms: None,
    }
}

/// T030: mock server with 5 tools, allow-list of 2, verify only 2 tools returned.
#[tokio::test]
async fn filter_allow_list_includes_only_matching_tools() {
    let mock_cfg = five_tool_config();
    let service = common::spawn_mock_server_with_client(&mock_cfg).await;

    let config = stub_config(
        "allow-test",
        Some(ToolFilter {
            allow: Some(vec!["tool_a".into(), "tool_b".into()]),
            deny: None,
        }),
    );

    let conn = McpConnection::from_service(config, service, None)
        .await
        .expect("connection should succeed");

    let manager =
        McpManager::from_connections(vec![conn]).expect("manager creation should succeed");
    let tools = manager.tools();

    let names: Vec<_> = tools.iter().map(|tool| tool.name().to_string()).collect();
    assert_eq!(names, vec!["tool_a".to_string(), "tool_b".to_string()]);
}

/// T031: mock server with 5 tools, deny-list of 1, verify 4 tools returned.
#[tokio::test]
async fn filter_deny_list_excludes_matching_tools() {
    let mock_cfg = five_tool_config();
    let service = common::spawn_mock_server_with_client(&mock_cfg).await;

    let config = stub_config(
        "deny-test",
        Some(ToolFilter {
            allow: None,
            deny: Some(vec!["tool_c".into()]),
        }),
    );

    let conn = McpConnection::from_service(config, service, None)
        .await
        .expect("connection should succeed");

    let manager =
        McpManager::from_connections(vec![conn]).expect("manager creation should succeed");
    let tools = manager.tools();

    let names: Vec<_> = tools.iter().map(|tool| tool.name().to_string()).collect();
    assert_eq!(names.len(), 4);
    assert!(!names.contains(&"tool_c".to_string()));
}

/// T032: allow applied first, then deny on a 5-tool server.
#[tokio::test]
async fn filter_allow_then_deny_applied_in_order() {
    let mock_cfg = five_tool_config();
    let service = common::spawn_mock_server_with_client(&mock_cfg).await;

    let config = stub_config(
        "combined-filter-test",
        Some(ToolFilter {
            allow: Some(vec!["tool_a".into(), "tool_b".into(), "tool_c".into()]),
            deny: Some(vec!["tool_c".into()]),
        }),
    );

    let conn = McpConnection::from_service(config, service, None)
        .await
        .expect("connection should succeed");

    let manager =
        McpManager::from_connections(vec![conn]).expect("manager creation should succeed");
    let tools = manager.tools();

    let names: Vec<_> = tools.iter().map(|tool| tool.name().to_string()).collect();
    assert_eq!(names, vec!["tool_a".to_string(), "tool_b".to_string()]);
}

/// T034: End-to-end with no filter — all discovered tools are returned.
#[tokio::test]
async fn no_filter_returns_all_tools() {
    let mock_cfg = five_tool_config();
    let service = common::spawn_mock_server_with_client(&mock_cfg).await;

    let config = stub_config("no-filter-test", None);

    let conn = McpConnection::from_service(config, service, None)
        .await
        .expect("connection should succeed");

    let manager =
        McpManager::from_connections(vec![conn]).expect("manager creation should succeed");
    let tools = manager.tools();

    let names: Vec<_> = tools.iter().map(|t| t.name().to_string()).collect();
    assert!(
        names
            == vec![
                "echo".to_string(),
                "tool_a".to_string(),
                "tool_b".to_string(),
                "tool_c".to_string(),
                "tool_d".to_string(),
            ],
        "no filter should return the full configured tool set, got: {names:?}"
    );
}
