# AGENTS.md — swink-agent-mcp

## Scope

`mcp/` — MCP (Model Context Protocol) integration. Discovered tools become `AgentTool` instances alongside native tools.

## Key Facts

- `McpManager` owns connections; `connect_all()` then `tools()` yields `Vec<Arc<McpTool>>`.
- `McpServerConfig`: name, transport, optional `tool_prefix`/`ToolFilter`, `requires_approval`, per-server timeouts.
- `McpTransport`: `Stdio { command, args, env }`, SSE, or streamable HTTP.
- `McpTool` uses provider-safe `"{prefix}_{tool_name}"` names; dispatch uses original server-advertised name.

## Key Invariants

- `shutdown()` operates on shared connection state (not `Arc::try_unwrap`) since exported `McpTool` handles retain clones.
- `connect_all()` stages connections locally; on `ToolNameCollision`, shuts staged connections before returning.
- Tool discovery at `connect_all()` time only — `reconnect()` to rediscover.
- Registration names go through same sanitizer/length cap as plugin tools.
- Stdio subprocesses start from `Command::env_clear()` plus only configured `env` entries.
- SSE auth with token rotation needs custom `StreamableHttpClient` wrapper (rmcp stores `auth_header` statically).
- Per-server timeouts distinguish transport handshake from `list_all_tools()` discovery.
