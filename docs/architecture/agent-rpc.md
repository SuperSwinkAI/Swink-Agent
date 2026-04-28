# Agent RPC — Architecture

`swink-agent-rpc` wraps a `swink_agent::Agent` behind a Unix-domain socket using JSON-RPC 2.0 / NDJSON. It is a single-tenant, per-user daemon — not a multi-client service.

## Crate Layout

```
rpc/
├── src/
│   ├── lib.rs              # re-exports (AgentServer, AgentClient)
│   ├── jsonrpc/
│   │   ├── message.rs      # RawMessage, RpcError, RequestId
│   │   └── peer.rs         # JsonRpcPeer, PeerSender — reader/writer tasks
│   ├── dto.rs              # Wire DTOs and From/Into bridges to core types
│   ├── server.rs           # AgentServer — Unix accept loop, session handler
│   └── client.rs           # AgentClient — connect + prompt_text API
└── src/bin/
    └── swink_agentd.rs     # `swink-agentd` daemon binary
```

## Transport

Each message is a single JSON object terminated by `\n` (NDJSON). The maximum line length is 1 MiB; oversized lines close the connection. JSON-RPC 2.0 is used as-is: `id` present = request/response, `id` absent = notification.

## Server Lifecycle

1. `AgentServer::bind(path, factory)` — validates no existing socket, stores factory.
2. `AgentServer::serve()` — binds `UnixListener`, sets `0600` permissions, runs accept loop.
3. Per connection: peer-credential check → single-session gate → handshake → `run_session`.
4. `run_session` builds a fresh `Agent` from the factory, wires tool-approval callback via `with_approve_tool_async`, then dispatches `prompt` / `cancel` / `shutdown`.
5. `run_prompt` drives `agent.prompt_stream(...)`, forwarding each `AgentEvent` as an `agent.event` notification. Cancel mid-turn is handled by `agent.abort()` at the next stream boundary.
6. `SocketCleanup` Drop guard removes the socket file on exit. Ctrl-C triggers graceful shutdown.

## Client Lifecycle

1. `AgentClient::connect(path)` — opens `UnixStream`, wraps in `JsonRpcPeer`, completes handshake.
2. `prompt_text(text)` drives `run_turn`: sends `prompt` request, polls `recv_incoming()` for `agent.event` and `tool.approve` messages until the `prompt` response arrives.
3. Tool-approval handler (optional, default auto-approve) is invoked synchronously for each `tool.approve` request.
4. `cancel()` / `shutdown()` fire notifications.

## Tool Approval Round-Trip

```
client                              server
  |  prompt → (request)               |
  |                                   |  agent running...
  |  ← tool.approve (request)         |
  |  tool.approve response →          |
  |                                   |  agent continues...
  |  ← agent.event (notification)*N   |
  |  ← prompt response                |
```

The server-side approval callback is captured from `peer.sender()` before `Agent::new(options)`, so it holds `Arc<PeerInner>` independently of the session borrow.

## Security

- Socket permissions: `0600` — only the owner can connect.
- Peer-credential check: Linux uses `SO_PEERCRED` via `nix::sys::socket::getsockopt`; macOS uses `getpeereid`. Connections from other effective UIDs are rejected before the handshake.
- Single-session enforcement: `Arc<AtomicBool>` guards against a second concurrent client. Second connections receive error code `-32098` and are closed immediately.

## Relationship to HLD

Per `HLD.md §11` ("library, not a service"), `swink-agentd` is a per-user adapter that exposes the library over a socket — it is not a shared daemon. Each user runs their own instance. This is consistent with the in-process TUI model; the two are complementary, not competing.

## Platform Support

The server and Unix-socket client are `#[cfg(unix)]` only. The workspace still builds on Windows because the `server` and `cli` features have unix-only code paths and the non-unix stubs return descriptive errors. The TUI's `InProcessTransport` continues to work on all platforms.
