# Research: JSON-RPC Agent Service

**Date**: 2026-04-27 | **Status**: Complete

## Wire Protocol Choice

**Decision**: JSON-RPC 2.0 over NDJSON (newline-delimited JSON), one compact message per line, 1 MiB max line length.
**Rationale**: JSON-RPC 2.0 is a lightweight, well-understood standard with bidirectional request/notification semantics that maps directly to the agent interaction model: client sends requests (prompt), server sends notifications (agent events) and requests (tool approval). NDJSON framing is trivial to implement, debuggable with standard tools (`nc`, `jq`), and avoids the complexity of length-prefix framing or HTTP/SSE.
**Alternatives considered**: gRPC — adds protobuf compilation step, heavier dependency; MCP (Model Context Protocol) — wrong abstraction level, designed for tool servers not agent sessions; raw TCP with custom framing — non-standard, harder to debug.

## Transport: Unix Socket vs TCP

**Decision**: Unix domain socket only. No TCP, no HTTP.
**Rationale**: The daemon is single-tenant, per-user, local-only. Unix sockets provide: (1) filesystem-based access control (mode 0600), (2) peer credential checking via kernel syscalls, (3) no network exposure. TCP would require TLS and authentication, adding complexity with no benefit for the local-only use case.
**Alternatives considered**: TCP with localhost binding — still requires auth, accidentally bindable to 0.0.0.0; HTTP/SSE — adds hyper dependency for a local-only service; named pipes on Windows — out of scope per user decision.

## Peer Credential Verification

**Decision**: Use `nix` crate for `SO_PEERCRED` (Linux) and `getpeereid` (macOS). Reject connections from different effective UIDs. Log-and-allow fallback on unsupported Unix variants.
**Rationale**: Socket file permissions (0600) are the primary access control but can be circumvented if the socket is in a shared directory or if the file is copied. Peer credential checking adds defense-in-depth at the kernel level. The `nix` crate (already in workspace dependencies) provides safe wrappers via `getsockopt` on Linux and `getpeereid` on macOS — both accept `AsFd` which `tokio::net::UnixStream` implements.
**Alternatives considered**: No credential check (rely on filesystem only) — insufficient for defense-in-depth; custom `libc` calls — `nix` already wraps them safely.

## Session Model: Factory vs Shared Agent

**Decision**: The server holds an `Arc<dyn Fn() -> AgentOptions>` factory. Each accepted connection creates a fresh `Agent` from the factory output.
**Rationale**: `Agent` is not `Clone` and holds internal state (conversation history, cancellation tokens). Creating a fresh agent per session provides clean isolation. The factory pattern lets the server be configured once but create agents on demand.
**Alternatives considered**: Shared `Arc<Mutex<Agent>>` — forces serialized access and carries stale conversation state between sessions; `AgentOptions: Clone` — options include closures (`StreamFn`) which aren't `Clone`.

## Tool Approval Callback Wiring

**Decision**: Extract `peer.sender()` (a `Clone`-able `PeerSender`) before building the `Agent`. Capture it in `AgentOptions::with_approve_tool_async`. The callback sends a `tool.approve` JSON-RPC request to the client and awaits the response.
**Rationale**: The approval callback must be set on `AgentOptions` before `Agent::new()` is called. `PeerSender` holds `Arc<PeerInner>` and the writer task runs independently, so the callback closure doesn't borrow the peer — it owns a cloned sender handle.
**Alternatives considered**: Setting approval callback after agent creation — not supported by the API; passing the peer into the agent — would require `Agent` to know about JSON-RPC.

## Concurrent Stream + Cancel

**Decision**: `tokio::select!` in `run_prompt` polls both `stream.next()` and `peer.recv_incoming()` concurrently. When a cancel notification arrives, `agent.abort()` is called. The stream drains to completion (abort causes the stream to end).
**Rationale**: `Agent::prompt_stream()` returns a `'static` stream (holds `Arc` refs into agent state, not `&mut Agent`), so the agent is not mutably borrowed while the stream is live. This allows `agent.abort()` to be called concurrently via the stored `CancellationToken`.
**Alternatives considered**: Sequential poll-then-check — would delay cancel processing until the next event; spawning stream consumption in a separate task — adds complexity without benefit since `select!` handles it naturally.

## Single-Session Enforcement

**Decision**: `Arc<AtomicBool>` shared across accept loop. `compare_exchange` on connection accept. Reset to `false` when session ends.
**Rationale**: Simple, lock-free, and correct. The agent loop is inherently single-threaded per session — multiple concurrent prompts would corrupt conversation state. Rejected connections get an immediate `session in use` error.
**Alternatives considered**: Semaphore with permit count 1 — heavier than needed; mutex around the accept loop — blocks other connections from getting an error response.

## Platform Gating Strategy

**Decision**: Server code, client Unix-socket code, and peer-credential code behind `#[cfg(unix)]`. Non-unix stubs return descriptive `RpcError::unavailable(...)`. The binary main is `#[cfg(unix)]` with a non-unix `main()` that prints an error and exits.
**Rationale**: The workspace must build on Windows for the TUI. Feature gates at the module level keep the code clean — no `#[cfg]` sprinkled through function bodies.
**Alternatives considered**: Entire crate behind `#[cfg(unix)]` — would require conditional workspace membership; runtime checks — wastes compilation time on dead code paths.
