//! Mock MCP server helpers for testing.
//!
//! Provides utilities to spawn in-process mock MCP servers that advertise
//! configurable tools and return configurable results.

#![allow(dead_code, clippy::unused_self, clippy::missing_const_for_fn)]

use std::collections::HashMap;
use std::sync::Arc;

use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, tool};
use serde_json::Value;

/// Configuration for a single mock tool.
#[derive(Debug, Clone)]
pub struct MockToolDef {
    /// Tool name as advertised by the mock server.
    pub name: String,
    /// Tool description.
    pub description: String,
    /// JSON Schema for the tool's input parameters.
    pub input_schema: Value,
    /// The result text to return when this tool is called.
    pub result_text: String,
    /// Whether the result should be marked as an error.
    pub is_error: bool,
}

impl MockToolDef {
    /// Create a simple mock tool that returns text.
    pub fn simple(name: &str, result_text: &str) -> Self {
        Self {
            name: name.to_string(),
            description: format!("Mock tool: {name}"),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
            }),
            result_text: result_text.to_string(),
            is_error: false,
        }
    }

    /// Create a mock tool with a specified input schema.
    pub fn with_schema(name: &str, schema: Value, result_text: &str) -> Self {
        Self {
            name: name.to_string(),
            description: format!("Mock tool: {name}"),
            input_schema: schema,
            result_text: result_text.to_string(),
            is_error: false,
        }
    }

    /// Create a mock tool that returns an error.
    pub fn error(name: &str, error_text: &str) -> Self {
        Self {
            name: name.to_string(),
            description: format!("Mock error tool: {name}"),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
            }),
            result_text: error_text.to_string(),
            is_error: true,
        }
    }
}

/// Configuration for a mock MCP server.
#[derive(Debug, Clone)]
pub struct MockServerConfig {
    /// Tools to advertise.
    pub tools: Vec<MockToolDef>,
    /// Custom tool results keyed by tool name.
    pub custom_results: HashMap<String, MockToolDef>,
}

impl MockServerConfig {
    /// Create a mock server config with the given tools.
    pub fn new(tools: Vec<MockToolDef>) -> Self {
        Self {
            tools,
            custom_results: HashMap::new(),
        }
    }

    /// Create an empty mock server config (no tools).
    pub fn empty() -> Self {
        Self {
            tools: Vec::new(),
            custom_results: HashMap::new(),
        }
    }
}

/// A mock MCP server that advertises configurable tools.
///
/// Uses rmcp's tool macro system to implement `ServerHandler`.
#[derive(Debug, Clone)]
pub struct MockMcpServer {
    /// Map of tool name to (result text, is error).
    results: Arc<HashMap<String, (String, bool)>>,
}

impl MockMcpServer {
    pub fn from_config(config: &MockServerConfig) -> Self {
        let mut results = HashMap::new();
        for tool_def in &config.tools {
            results.insert(
                tool_def.name.clone(),
                (tool_def.result_text.clone(), tool_def.is_error),
            );
        }
        Self {
            results: Arc::new(results),
        }
    }
}

#[tool(tool_box)]
impl MockMcpServer {
    /// A generic echo tool for testing — returns whatever text it receives.
    #[tool(description = "Echo the input back")]
    fn echo(&self, #[tool(param)] text: String) -> String {
        text
    }
}

#[tool(tool_box)]
impl ServerHandler for MockMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("Mock MCP server for testing".into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..ServerInfo::default()
        }
    }
}

/// Spawn an in-process MCP server and connect a client, returning the running client service.
///
/// Uses `tokio::io::duplex()` to create an in-memory bidirectional channel.
pub async fn spawn_mock_server_with_client(
    config: &MockServerConfig,
) -> rmcp::service::RunningService<rmcp::service::RoleClient, rmcp::model::ClientInfo> {
    use rmcp::service::ServiceExt;

    let server = MockMcpServer::from_config(config);

    // Create in-memory duplex streams.
    let (client_stream, server_stream) = tokio::io::duplex(4096);

    // Spawn the server on one end of the duplex.
    let _server_handle = tokio::spawn(async move {
        let _ = server.serve(server_stream).await;
    });

    // Connect the client on the other end.
    let client_info = rmcp::model::ClientInfo::default();
    client_info
        .serve(client_stream)
        .await
        .expect("client connection should succeed")
}

/// Spawn an in-process MCP server and return a fully-formed `McpConnection`.
///
/// This creates an in-memory duplex, spawns the mock server, connects a client,
/// and wraps it in an `McpConnection` with tool discovery already performed.
pub async fn spawn_mock_connection(
    server_name: &str,
    tool_prefix: Option<&str>,
    mock_tools: Vec<MockToolDef>,
) -> swink_agent_mcp::McpConnection {
    let mock_config = MockServerConfig::new(mock_tools);
    let service = spawn_mock_server_with_client(&mock_config).await;

    let mcp_config = swink_agent_mcp::McpServerConfig {
        name: server_name.to_string(),
        transport: swink_agent_mcp::McpTransport::Stdio {
            command: "mock".into(),
            args: vec![],
            env: HashMap::default(),
        },
        tool_prefix: tool_prefix.map(String::from),
        tool_filter: None,
        requires_approval: false,
    };

    swink_agent_mcp::McpConnection::from_service(mcp_config, service)
        .await
        .expect("mock connection should succeed")
}
