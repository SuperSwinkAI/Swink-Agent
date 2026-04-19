# Data Model: MCP Integration

**Feature**: 038-mcp-integration
**Date**: 2026-04-01

## Entities

### McpTransport

Describes how to connect to an MCP server.

| Variant | Fields | Description |
|---------|--------|-------------|
| Stdio | `command: String`, `args: Vec<String>`, `env: HashMap<String, String>` | Subprocess spawned with stdin/stdout communication. `env` merges with (and overrides) parent process environment. |
| Sse | `url: String`, `bearer_token: Option<String>`, `headers: HashMap<String, String>` | HTTP connection using Server-Sent Events. Optional bearer token for `Authorization` plus additional custom headers on every request. |

### McpServerConfig

Configuration for a single MCP server connection.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| name | String | Yes | Unique identifier for this server (used in events and error messages). |
| transport | McpTransport | Yes | How to connect (Stdio or Sse). |
| tool_prefix | Option\<String\> | No | If set, all tool names from this server are prefixed with `{prefix}_`. |
| tool_filter | Option\<ToolFilter\> | No | Controls which discovered tools are exposed to the agent. |
| requires_approval | bool | No (default: true) | Whether tools from this server require user approval before execution. |

**Identity**: Unique by `name`. No two configs may share the same name.

### ToolFilter

Controls which tools from an MCP server are exposed.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| allow | Option\<Vec\<String\>\> | No | If set, only tools with names in this list are included. |
| deny | Option\<Vec\<String\>\> | No | If set, tools with names in this list are excluded. |

**Evaluation order**: Allow-list applied first (if present), then deny-list filters the result.

### McpConnection

Runtime state for an active MCP server connection.

| Field | Type | Description |
|-------|------|-------------|
| config | McpServerConfig | The configuration that produced this connection. |
| session | RunningService (rmcp) | The active client session (internal, not exposed). |
| tools | Vec\<McpTool\> | Tools discovered from this server after filtering. |
| status | McpConnectionStatus | Current connection health. |

**Lifecycle**:
- Created → Connected (tools discovered) → Active (tool calls in progress) → Disconnected (error or shutdown)
- Transition to Disconnected marks all tools as unavailable.

### McpConnectionStatus

| Variant | Description |
|---------|-------------|
| Connected | Server is reachable and tools are available. |
| Disconnected { reason: String } | Server is unreachable. Tools from this server are unavailable. |

### McpTool

A discovered tool from an MCP server, implementing `AgentTool`.

| Field | Type | Description |
|-------|------|-------------|
| name | String | Tool name as exposed to the LLM (may include prefix). |
| original_name | String | Tool name as reported by the MCP server (without prefix). |
| description | String | Human-readable description from the MCP server. |
| input_schema | Value | JSON Schema for tool parameters from the MCP server. |
| connection | Arc\<McpConnection\> | Reference to the parent connection for routing calls. |
| server_name | String | Name of the originating server (for events and diagnostics). |

**Identity**: Unique by `name` within an agent's tool set. Collisions are detected at connection time.

### McpManager

Orchestrates multiple MCP server connections.

| Field | Type | Description |
|-------|------|-------------|
| connections | Vec\<McpConnection\> | Active connections (successful only). |
| all_tools | Vec\<Arc\<McpTool\>\> | Flattened list of all tools across all connections. |

**Lifecycle**: Created with configs → `connect_all()` establishes connections → tools extracted → manager owned by agent → dropped on agent shutdown.

## State Transitions

```
McpConnection Lifecycle:

  [Config provided]
        │
        ▼
  ┌─────────────┐    connection failed     ┌──────────────┐
  │  Connecting  │ ───────────────────────► │ Disconnected │
  └─────────────┘                           └──────────────┘
        │                                          ▲
        │ success                                  │
        ▼                                          │
  ┌─────────────┐    subprocess crash /            │
  │  Connected   │    network error ───────────────┘
  └─────────────┘
        │
        │ agent shutdown / cancellation
        ▼
  ┌─────────────┐
  │   Dropped    │  (subprocess terminated, resources freed)
  └─────────────┘
```

## Relationships

```
McpManager 1──* McpConnection
McpConnection 1──1 McpServerConfig
McpConnection 1──* McpTool
McpTool *──1 McpConnection (Arc reference for call routing)
McpServerConfig 1──0..1 ToolFilter
Agent 1──0..1 McpManager
Agent 1──* AgentTool (McpTool implements AgentTool)
```
