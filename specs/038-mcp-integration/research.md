# Research: MCP Integration

**Feature**: 038-mcp-integration
**Date**: 2026-04-01

## R1: Rust MCP Client Crate Selection

**Decision**: Use `rmcp` (official MCP SDK by the MCP org)

**Rationale**:
- Published by the `modelcontextprotocol` GitHub organization — ensures spec compliance.
- Built on `tokio` and `tower`, matching Swink's async runtime.
- Supports stdio and SSE transports out of the box.
- Provides `TokioChildProcessBuilder` for subprocess spawning with configurable stdio/stderr.
- `Peer::list_tools()` and `Peer::call_tool()` map directly to our discovery and execution needs.
- `Content` enum handles text, images, and structured data.
- Active development, highest download count among Rust MCP crates.

**Alternatives Considered**:
- `mcp-sdk`: Community crate, earlier entry. Less actively maintained after `rmcp` appeared. Lower spec compliance guarantees.
- `mcp-client` / `mcp-rs`: Smaller community efforts. Low download counts (<1k/month). Varying spec compliance.
- Hand-rolled JSON-RPC: Would provide full control but massive scope increase. The MCP protocol has nuances (capability negotiation, pagination, cancellation) that `rmcp` handles.

## R2: Crate Placement — New Crate vs. Existing

**Decision**: New workspace crate `swink-agent-mcp`

**Rationale**:
- `rmcp` brings ~15 transitive dependencies (tower, hyper, etc.) that should not be forced on consumers who don't use MCP.
- MCP implements `AgentTool` (tool provider), not `StreamFn` (LLM adapter) — it doesn't belong in adapters.
- Follows established pattern: adapters, policies, auth, memory are all separate crates with their own dependency trees.
- Feature-gating within core would still compile `rmcp` code paths and bloat the core crate.

**Alternatives Considered**:
- In `swink-agent` core behind `feature = "mcp"`: Rejected — pollutes core's dependency tree. Core's principle is to remain free of protocol-specific dependencies.
- In `swink-agent-adapters`: Rejected — adapters implement `StreamFn` for LLM streaming. MCP is a tool protocol, not a provider protocol. Different trait boundary.

## R3: AgentTool Implementation Strategy

**Decision**: Each discovered MCP tool becomes a separate `McpTool` struct implementing `AgentTool`. The struct holds an `Arc` reference to its parent `McpConnection` for routing calls.

**Rationale**:
- `AgentTool` requires `name()`, `description()`, `parameters_schema()`, and `execute()` — all directly mappable from MCP's `Tool` definition.
- MCP `Tool.name` → `AgentTool::name()`, `Tool.description` → `AgentTool::description()`, `Tool.input_schema` → `AgentTool::parameters_schema()`.
- `execute()` forwards to `connection.peer().call_tool(params)` and converts `CallToolResult` to `AgentToolResult`.
- `McpTool` sets `metadata()` to `ToolMetadata::with_namespace("mcp")` (or the configured prefix) for identification.
- `requires_approval()` defaults to `true` for MCP tools — they execute external code. Configurable per-server.

**Alternatives Considered**:
- Single "MCP dispatcher" tool that routes by name: Rejected — the LLM needs individual tool definitions with distinct schemas to make good tool choices. A dispatcher would hide tool schemas.

## R4: Connection Lifecycle Management

**Decision**: `McpManager` owns all connections. Each `McpConnection` wraps an `rmcp` `RunningService` (client session). For stdio, the `TokioChildProcess` is owned by the connection and dropped on cleanup. For SSE, the connection uses `rmcp`'s built-in HTTP transport.

**Rationale**:
- `rmcp`'s `TokioChildProcess` has built-in `ChildWithCleanup` that terminates the subprocess on drop.
- `McpManager::connect_all()` is called during agent setup (before the loop starts). It spawns connections concurrently and collects discovered tools.
- Connections that fail are logged and skipped — the manager returns a partial tool set.
- `McpManager` implements `Drop` to ensure all connections are cleaned up.

**Alternatives Considered**:
- Lazy connection (connect on first tool call): Rejected — adds unpredictable latency during conversation. Better to fail fast at startup.
- Consumer-managed lifecycle: Rejected — error-prone, zombie process risk. The spec explicitly requires automatic lifecycle management.

## R5: Event Integration

**Decision**: Add new `AgentEvent` variants for MCP lifecycle events. Events are emitted via the existing `dispatch_event` mechanism.

**Rationale**:
- The `AgentEvent` enum is `#[non_exhaustive]`, so adding variants is backward-compatible.
- New variants: `McpServerConnected { server_name }`, `McpServerDisconnected { server_name, reason }`, `McpToolsDiscovered { server_name, tool_count }`, `McpToolCallStarted { server_name, tool_name }`, `McpToolCallCompleted { server_name, tool_name, is_error }`.
- Events use existing subscriber infrastructure — no new event system needed.

**Alternatives Considered**:
- Separate MCP event channel: Rejected — duplicates infrastructure. The existing event system is designed for this.
- Log-only (tracing): Rejected — tracing is for diagnostics. Events are for programmatic observation (UI, audit, plugins).

## R6: SSE Authentication

**Decision**: `McpServerConfig` includes an optional `bearer_token` field for SSE connections. At connection time, if a bearer token is configured, it's passed as an `Authorization: Bearer <token>` header. The token can be sourced from the credential resolver (spec 035) if available.

**Rationale**:
- Bearer tokens cover the vast majority of remote MCP server auth (API keys, OAuth2 tokens).
- `rmcp`'s SSE transport supports custom headers.
- Integration with the credential resolver allows tokens to be refreshed automatically.

**Alternatives Considered**:
- No auth: Rejected — remote servers almost always require authentication.
- Full auth suite (mTLS, custom headers): Over-scoped for initial integration. Bearer token + credential resolver covers 95% of use cases.

## R7: Type Conversion Strategy

**Decision**: Dedicated `convert` module maps between `rmcp` types and `swink-agent` types.

**Rationale**:
- `rmcp::Content::Text { text }` → `ContentBlock::Text { text }` (direct mapping).
- `rmcp::Content::Image { data, mime_type }` → `ContentBlock::Image { source }` (base64 data).
- Other `rmcp::Content` variants → `ContentBlock::Text { text: "[unsupported content type: ...]" }` (graceful fallback).
- `CallToolResult { content, is_error }` → `AgentToolResult { content, details: Null, is_error }`.
- `CallToolRequestParams { name, arguments }` ← constructed from `AgentTool::execute()` params.
- Conversion is infallible — no `Result` types needed. Unsupported content degrades gracefully.

**Alternatives Considered**:
- Direct use of `rmcp` types in public API: Rejected — leaks `rmcp` dependency to consumers. The MCP crate's public API uses only `swink-agent` types.
