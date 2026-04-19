# Quickstart: MCP Integration

**Feature**: 038-mcp-integration

## Add Dependency

```toml
# In your Cargo.toml
[dependencies]
swink-agent = "0.4"
swink-agent-mcp = "0.1"
```

## Basic Usage — Connect to a Stdio MCP Server

```rust
use std::sync::Arc;
use swink_agent::{Agent, AgentOptions};
use swink_agent_mcp::{McpManager, McpServerConfig, McpTransport};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure an MCP server (filesystem tools via npx)
    let config = McpServerConfig {
        name: "filesystem".into(),
        transport: McpTransport::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "@modelcontextprotocol/server-filesystem".into(), "/tmp".into()],
            env: Default::default(),
        },
        tool_prefix: Some("fs".into()),
        tool_filter: None,
        requires_approval: true,
    };

    // Connect and discover tools
    let mut mcp = McpManager::new(vec![config]);
    mcp.connect_all().await?;

    // Build agent with MCP tools
    let mut options = AgentOptions::new();
    for tool in mcp.tools() {
        options.tools.push(tool);
    }

    // ... configure stream_fn and run agent loop
    // MCP tools appear alongside native tools — the LLM uses them transparently

    // Cleanup happens automatically when McpManager is dropped
    Ok(())
}
```

## Multiple Servers with Filtering

```rust
let configs = vec![
    McpServerConfig {
        name: "database".into(),
        transport: McpTransport::Stdio {
            command: "mcp-server-postgres".into(),
            args: vec!["--connection-string".into(), "postgres://localhost/mydb".into()],
            env: [("PGPASSWORD".into(), "secret".into())].into(),
        },
        tool_prefix: Some("db".into()),
        tool_filter: Some(ToolFilter {
            allow: Some(vec!["query".into(), "list_tables".into()]),
            deny: None,
        }),
        requires_approval: true,
    },
    McpServerConfig {
        name: "web_search".into(),
        transport: McpTransport::Sse {
            url: "https://mcp.example.com/sse".into(),
            bearer_token: Some("sk-...".into()),
            headers: [("x-api-key".into(), "api-key-123".into())].into(),
        },
        tool_prefix: None,
        tool_filter: None,
        requires_approval: false,
    },
];

let mut mcp = McpManager::new(configs);
mcp.connect_all().await?;
// "database" server contributes: db_query, db_list_tables
// "web_search" server contributes its tools with original names
```

## Event Observation

MCP lifecycle events flow through the standard `AgentEvent` system:

```rust
agent.subscribe(|event| {
    match event {
        AgentEvent::McpServerConnected { server_name } => {
            println!("Connected to MCP server: {server_name}");
        }
        AgentEvent::McpToolsDiscovered { server_name, tool_count } => {
            println!("Discovered {tool_count} tools from {server_name}");
        }
        AgentEvent::McpToolCallStarted { server_name, tool_name } => {
            println!("Calling {tool_name} on {server_name}");
        }
        _ => {}
    }
});
```
