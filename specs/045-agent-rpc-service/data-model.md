# Data Model: JSON-RPC Agent Service

## Types Defined in This Crate

### AgentServer (struct)

Hosts an agent behind a Unix socket. Configured with a factory that creates fresh agent options per session.

| Field | Type | Description |
|-------|------|-------------|
| `path` | `PathBuf` | Unix socket file path |
| `factory` | `Arc<dyn Fn() -> AgentOptions + Send + Sync>` | Creates fresh agent configuration per accepted connection |

Constructors: `bind(path, factory)` (fails if socket exists), `bind_force(path, factory)` (removes existing socket first).
Methods: `serve()` — starts the accept loop, runs until shutdown signal.

### AgentClient (struct)

Connects to a running server and drives agent turns.

| Field | Type | Description |
|-------|------|-------------|
| `peer` | `JsonRpcPeer` | The underlying transport peer |
| `approval_handler` | `Option<Box<dyn Fn(ToolApprovalRequest) -> ToolApproval + Send + Sync>>` | Optional handler for tool approval requests |

Constructors: `connect(path)` — connects and completes handshake.
Builder: `with_approval_handler(handler)` — sets the approval callback.
Methods: `prompt_text(text)` — sends prompt, collects events; `cancel()` — aborts current turn; `shutdown()` — closes session.

### JsonRpcPeer (struct)

Transport-agnostic JSON-RPC 2.0 peer over any async reader/writer pair. Spawns reader and writer tasks.

| Field | Type | Description |
|-------|------|-------------|
| `sender` | `PeerSender` | Cloneable write handle |
| `incoming_rx` | `mpsc::Receiver<IncomingMessage>` | Channel for incoming requests and notifications |

Constructor: `new(read, write)` — generic over `AsyncRead + Unpin + Send + 'static` and `AsyncWrite + Unpin + Send + 'static`.
Methods: `sender()` — returns a clone of the sender handle; `recv_incoming()` — awaits the next incoming message.

### PeerSender (struct, Clone)

Cloneable write handle for sending messages. Safe to capture in closures and share across tasks.

| Field | Type | Description |
|-------|------|-------------|
| `inner` | `Arc<PeerInner>` | Shared state: outbound channel, pending request map, ID counter |

Methods: `notify(method, params)`, `request(method, params) -> Result<R>`, `respond_ok(id, result)`, `respond_err(id, error)`.

### RawMessage (struct)

Flat JSON-RPC 2.0 message envelope. Classification determined by which optional fields are present.

| Field | Type | Description |
|-------|------|-------------|
| `jsonrpc` | `String` | Must be "2.0" |
| `id` | `Option<RequestId>` | Present in requests/responses; absent in notifications |
| `method` | `Option<String>` | Present in requests/notifications; absent in responses |
| `params` | `Option<Value>` | Request/notification parameters |
| `result` | `Option<Value>` | Successful response payload |
| `error` | `Option<RpcError>` | Error response payload |

Constructors: `request(id, method, params)`, `notification(method, params)`, `success(id, result)`, `error_response(id, error)`.

### RequestId (enum)

JSON-RPC 2.0 request identifier.

| Variant | Payload | Description |
|---------|---------|-------------|
| `Number` | `u64` | Numeric identifier (default for client-generated IDs) |
| `Str` | `String` | String identifier (supported for interoperability) |

### RpcError (struct)

JSON-RPC 2.0 error object with standard and application-defined codes.

| Field | Type | Description |
|-------|------|-------------|
| `code` | `i64` | Error code |
| `message` | `String` | Human-readable message |
| `data` | `Option<Value>` | Optional structured error data |

Standard codes: `-32700` (parse), `-32600` (invalid request), `-32601` (method not found), `-32602` (invalid params), `-32603` (internal).
Application codes: `-32099` (protocol mismatch), `-32098` (session in use), `-32097` (disconnected), `-32096` (unavailable).

### IncomingMessage (enum)

A message received from the remote peer, ready for application handling.

| Variant | Fields | Description |
|---------|--------|-------------|
| `Request` | `id, method, params` | Remote is requesting something; must respond |
| `Notification` | `method, params` | Remote is notifying; no response expected |

## Wire DTOs

### InitializeParams / InitializedParams (structs)

Handshake messages exchanged during connection setup.

| Field | Type | Description |
|-------|------|-------------|
| `protocol_version` | `String` | Must be "1.0" |
| `client` / `server` | `ClientInfo` / `ServerInfo` | Name and version of the peer |

### PromptParams / PromptResult (structs)

| Field | Type | Description |
|-------|------|-------------|
| `text` | `String` | (PromptParams) The prompt text |
| `session_id` | `Option<String>` | (PromptParams) Optional session identifier |
| `turn_id` | `String` | (PromptResult) Identifier for the completed turn |

### ToolApprovalRequestDto / ToolApprovalDto (structs)

Wire representations of tool approval, decoupled from core types via `From` impls.

| Field | Type | Description |
|-------|------|-------------|
| `id` | `String` | (Request) Tool call identifier |
| `name` | `String` | (Request) Tool name |
| `arguments` | `Value` | (Request) Tool arguments |
| `requires_approval` | `bool` | (Request) Whether approval is required |
| `context` | `Option<String>` | (Request) Additional context |
| `approved` | `bool` | (Response) Whether approved |
| `value` | `Option<Value>` | (Response) Modified value if approved-with-modification |

## State Transitions

### Server Session Lifecycle

```
[Listening] → accept → [Credential Check]
  → reject (wrong UID) → [Listening]
  → reject (session active) → [Listening]
  → pass → [Handshake]
    → receive initialize → send initialized → [Session Active]
      → receive prompt → [Streaming Turn]
        → stream events → send prompt result → [Session Active]
        → receive cancel → abort agent → [Session Active]
      → receive shutdown → [Session Ended] → [Listening]
      → disconnect → [Session Ended] → [Listening]
```

### Client Lifecycle

```
[Disconnected] → connect → send initialize → receive initialized → [Connected]
  → prompt_text → [Awaiting Turn]
    → receive agent.event* → receive prompt result → [Connected]
    → receive tool.approve → respond → continue → [Awaiting Turn]
  → cancel → [Connected]
  → shutdown → [Disconnected]
```
