# AGENTS.md — swink-agent-mcp

## Scope

`mcp/` — MCP (Model Context Protocol) integration. Discovered tools implement `AgentTool` and are usable alongside native tools in the agent loop.

## Key Facts

- `McpManager` — owns all server connections; call `connect_all()` then `tools()` to get `Vec<Arc<McpTool>>`.
- `McpServerConfig` — name, transport, optional `tool_prefix`, optional `ToolFilter`, `requires_approval` flag.
- `McpTransport` — `Stdio { command, args, env }`, SSE, or streamable HTTP (via `rmcp` features).
- `McpTool` — wraps a discovered MCP tool as `AgentTool`. Prefixed as `"{prefix}.{tool_name}"` when `tool_prefix` is set.
- `McpConnection` tracks per-server state and exposes `status()` for health checks.
- Uses `rmcp` SDK (v1.3) for the wire protocol.

## Lessons Learned

- `McpManager::shutdown()` must operate on shared connection-owned state, not `Arc::try_unwrap()`: exported `McpTool` handles keep `Arc<McpConnection>` clones alive, so deterministic disconnect has to clear the session/monitor even when callers still retain tool `Arc`s.
- Tool discovery happens at `connect_all()` time. Tools added to a server after connection are not auto-refreshed; call `reconnect()` to rediscover.

## Build & Test

```bash
cargo build -p swink-agent-mcp
cargo test -p swink-agent-mcp
cargo clippy -p swink-agent-mcp -- -D warnings
```
