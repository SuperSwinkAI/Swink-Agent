# swink-agent-mcp

[![Crates.io](https://img.shields.io/crates/v/swink-agent-mcp.svg)](https://crates.io/crates/swink-agent-mcp)
[![Docs.rs](https://docs.rs/swink-agent-mcp/badge.svg)](https://docs.rs/swink-agent-mcp)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](https://github.com/SuperSwinkAI/Swink-Agent/blob/main/LICENSE-MIT)

Model Context Protocol (MCP) integration for [`swink-agent`](https://crates.io/crates/swink-agent) — connect stdio and SSE servers, discover their tools, and call them from the agent loop as if they were native.

## Features

- **`McpManager`** — connect and manage many MCP servers; reconnect on failure
- **`McpTransport::Stdio`** — spawn a child process with args and env (MCP filesystem, git, GitHub, etc.)
- **`McpTransport::Http`** — streamable HTTP with bearer-token auth (`SseBearerAuth`)
- **Discovered tools implement `AgentTool`** — drop them into `AgentOptions::with_tools` alongside native tools
- **`tool_prefix`** namespaces discovered tools (e.g. `fs_read_file`) to prevent collisions
- **`ToolFilter`** opt-in allow-list for which discovered tools are exposed
- **`requires_approval`** per-server gate — route MCP calls through `ApprovalMode` before execution

## Quick Start

```toml
[dependencies]
swink-agent = "0.9.0"
swink-agent-mcp = "0.9.0"
tokio = { version = "1", features = ["full"] }
```

```rust,ignore
use swink_agent_mcp::{McpManager, McpServerConfig, McpTransport};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let configs = vec![McpServerConfig {
        name: "filesystem".into(),
        transport: McpTransport::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "@modelcontextprotocol/server-filesystem".into()],
            env: Default::default(),
    },
    tool_prefix: Some("fs".into()),
    tool_filter: None,
    requires_approval: true,
    connect_timeout_ms: Some(5_000),
    discovery_timeout_ms: Some(5_000),
}];

    let mut mcp = McpManager::new(configs);
    mcp.connect_all().await?;
    let tools = mcp.tools();  // Vec<Arc<dyn AgentTool>> — pass into AgentOptions
    Ok(())
}
```

## Architecture

Each server gets its own `McpConnection` wrapping an `rmcp` transport; `McpManager` holds them in a registry keyed by server name. Discovered tools are wrapped in `McpTool`, which implements `AgentTool` and translates between MCP's `CallToolRequest`/`CallToolResult` and `swink-agent`'s tool-call shape. Connection status is tracked per-server (`McpConnectionStatus`) so a broken server never takes down the others.

No `unsafe` code (`#![forbid(unsafe_code)]`). Child-process transports inherit no environment by default — you must list the env vars the server needs explicitly.

---

Part of the [swink-agent](https://github.com/SuperSwinkAI/Swink-Agent) workspace — see the [main README](https://github.com/SuperSwinkAI/Swink-Agent#readme) for workspace overview and setup.
