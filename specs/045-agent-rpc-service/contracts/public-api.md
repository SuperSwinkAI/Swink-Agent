# Public API Contract: swink-agent-rpc

## Crate Re-exports (`lib.rs`)

| Symbol | Feature Gate | Description |
|--------|-------------|-------------|
| `AgentServer` | `server` | Unix socket server hosting an agent |
| `AgentClient` | `client` | Client connecting to a remote agent |
| `jsonrpc` (module) | always | `JsonRpcPeer`, `PeerSender`, `RpcError`, `IncomingMessage`, `RawMessage`, `RequestId`, `MAX_LINE_BYTES` |
| `dto` (module) | always | Wire DTOs, method constants, `PROTOCOL_VERSION` |

## Wire Protocol Contract

### Framing

- NDJSON: one JSON-RPC 2.0 object per `\n`-terminated line
- Maximum line length: 1,048,576 bytes (1 MiB)
- Exceeding max → connection closed

### Handshake Sequence

```
Client → Server: {"jsonrpc":"2.0","method":"initialize","params":{"protocol_version":"1.0","client":{"name":"...","version":"..."}}}
Server → Client: {"jsonrpc":"2.0","method":"initialized","params":{"protocol_version":"1.0","server":{"name":"...","version":"..."}}}
```

### Methods

| Method | Direction | Type | Params | Result |
|--------|-----------|------|--------|--------|
| `initialize` | C→S | notification | `InitializeParams` | — |
| `initialized` | S→C | notification | `InitializedParams` | — |
| `prompt` | C→S | request | `PromptParams` | `PromptResult` |
| `agent.event` | S→C | notification | `AgentEvent` (core type) | — |
| `tool.approve` | S→C | request | `ToolApprovalRequestDto` | `ToolApprovalDto` |
| `cancel` | C→S | notification | `null` | — |
| `shutdown` | C→S | notification | `null` | — |

### Error Codes

| Code | Constant | Meaning |
|------|----------|---------|
| -32700 | `PARSE_ERROR` | Malformed JSON |
| -32600 | `INVALID_REQUEST` | Invalid JSON-RPC structure |
| -32601 | `METHOD_NOT_FOUND` | Unknown method |
| -32602 | `INVALID_PARAMS` | Invalid method parameters |
| -32603 | `INTERNAL_ERROR` | Server internal error |
| -32099 | `PROTOCOL_MISMATCH` | Incompatible protocol version |
| -32098 | `SESSION_IN_USE` | Another client already connected |
| -32097 | `DISCONNECTED` | Peer disconnected |
| -32096 | `UNAVAILABLE` | Feature not available on this platform |

## Server API Contract

```
AgentServer::bind(path, factory) -> io::Result<Self>
AgentServer::bind_force(path, factory) -> Self
AgentServer::serve(self) -> io::Result<()>      // #[cfg(unix)]
```

- `factory: impl Fn() -> AgentOptions + Send + Sync + 'static`
- `serve()` blocks until Ctrl-C or all connections close
- Socket file permissions set to 0600 on bind
- Socket file removed on drop (via `SocketCleanup` guard)
- Peer credential check on every accepted connection

## Client API Contract

```
AgentClient::connect(path) -> Result<Self, RpcError>     // #[cfg(unix)]
AgentClient::with_approval_handler(self, handler) -> Self
AgentClient::prompt_text(&mut self, text) -> Result<Vec<AgentEvent>, RpcError>
AgentClient::cancel(&self) -> Result<(), RpcError>
AgentClient::shutdown(self) -> Result<(), RpcError>
```

- `connect` performs handshake automatically
- `prompt_text` blocks until the turn completes, collecting all events
- `with_approval_handler` sets a synchronous callback for `tool.approve` requests
- Default (no handler): all tools auto-approved

## CLI Contract (`swink-agentd`)

```
swink-agentd [OPTIONS]

Options:
  -l, --listen <PATH>          Unix socket path [default: /tmp/swink.sock]
      --force                  Remove existing socket before binding
  -m, --model <MODEL>          Model to use [default: claude-sonnet-4-6]
  -s, --system-prompt <TEXT>   System prompt [default: "You are a helpful assistant."]
```

- Requires `cli` feature
- Unix-only; non-Unix prints error and exits with code 1
