# AGENTS.md — swink-agent-mcp

## Scope

`mcp/` — MCP (Model Context Protocol) integration. Discovered tools implement `AgentTool` and are usable alongside native tools in the agent loop.

## Key Facts

- `McpManager` — owns all server connections; call `connect_all()` then `tools()` to get `Vec<Arc<McpTool>>`.
- `McpServerConfig` — name, transport, optional `tool_prefix`, optional `ToolFilter`, `requires_approval` flag, and optional per-server `connect_timeout_ms` / `discovery_timeout_ms` bootstrap bounds.
- `McpTransport` — `Stdio { command, args, env }`, SSE, or streamable HTTP (via `rmcp` features).
- `McpTool` — wraps a discovered MCP tool as `AgentTool`. Registered names use provider-safe `"{prefix}_{tool_name}"` composition when `tool_prefix` is set; dispatch still uses the original server-advertised tool name.
- `McpConnection` tracks per-server state and exposes `status()` for health checks.
- Uses `rmcp` SDK (v1.3) for the wire protocol.

## Lessons Learned

- `McpManager::shutdown()` must operate on shared connection-owned state, not `Arc::try_unwrap()`: exported `McpTool` handles keep `Arc<McpConnection>` clones alive, so deterministic disconnect has to clear the session/monitor even when callers still retain tool `Arc`s.
- `McpManager::connect_all()` must stage successful connections locally until tool-collision checks pass. If discovery ends with `ToolNameCollision`, shut those staged connections down before returning so partial sessions are not left running.
- Explicit `McpServerDisconnected` events for intentional teardown come from `McpConnection::shutdown()`, not the monitor task. Tests that shut a connection down with an event channel must drain that lifecycle event before asserting later tool-call events.
- Tool discovery happens at `connect_all()` time. Tools added to a server after connection are not auto-refreshed; call `reconnect()` to rediscover.
- SSE recovery is delegated to rmcp's streamable HTTP transport: transient stream drops and stale-session 404s are retried/re-initialized inside rmcp, so `McpConnection` should only flip to `Disconnected` after the underlying service fully exits.
- rmcp stores SSE `auth_header` as a static string inside the transport config and reuses it for reconnect/re-init paths. Resolver-backed SSE auth that must survive token rotation therefore needs a custom `StreamableHttpClient` wrapper that resolves credentials per HTTP request instead of only at initial connect.
- MCP tool registration names must go through the same provider-safe sanitizer/length cap as plugin tools. `McpTool` keeps the raw `original_name` for outbound MCP calls, while `McpManager` collision detection runs on the sanitized registration name exposed to providers.
- Stdio MCP subprocesses must start from `Command::env_clear()` and then receive only configured `env` entries; inheriting the parent environment leaks unrelated secrets and tokens into child servers.
- Per-server MCP bootstrap bounds belong on `McpServerConfig`, and the timeout enforcement has to distinguish transport handshake from `list_all_tools()` discovery so `connect_all()` can skip one hung server without blocking later configs.

## Build & Test

```bash
cargo build -p swink-agent-mcp
cargo test -p swink-agent-mcp
cargo clippy -p swink-agent-mcp -- -D warnings
```
