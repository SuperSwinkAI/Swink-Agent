# Implementation Plan: JSON-RPC Agent Service

**Branch**: `045-agent-rpc-service` | **Date**: 2026-04-27 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/045-agent-rpc-service/spec.md`

## Summary

New workspace crate `swink-agent-rpc` exposing a `swink_agent::Agent` over a Unix-domain socket using JSON-RPC 2.0 / NDJSON. The crate ships a server (hosts an agent behind a Unix socket with peer-credential security and single-session enforcement), a Rust client (connects, sends prompts, receives streamed agent events, handles tool approvals), and a `swink-agentd` daemon binary. The TUI is unchanged — this is a standalone service layer for external consumers.

## Technical Context

**Language/Version**: Rust latest stable (edition 2024)
**Primary Dependencies**: `swink-agent` (core types — `Agent`, `AgentOptions`, `AgentEvent`, `ToolApproval`, `ToolApprovalRequest`), `tokio` (async runtime — net, io-util, macros, rt-multi-thread, sync, signal), `serde`/`serde_json` (wire serialization), `futures`/`tokio-stream` (stream combinators), `nix` (peer credential checks — `socket`, `user` features), `clap` (CLI binary), `tracing` (observability)
**Storage**: N/A — stateless; no persistence beyond the socket file
**Testing**: `cargo test --workspace` — unit tests for JSON-RPC peer, integration tests for end-to-end server/client lifecycle
**Target Platform**: Unix (Linux, macOS) for server/client; workspace compiles on Windows with stub code paths
**Project Type**: Library crate (`swink-agent-rpc`) + binary (`swink-agentd`)
**Performance Goals**: Handshake completes in under 100ms. Event streaming adds negligible latency over the agent's own event emission. Round-trip tool approval under 2 seconds.
**Constraints**: `#[forbid(unsafe_code)]`; depends only on `swink-agent` public APIs; single-session server; Unix-only wire transport
**Scale/Scope**: New crate — ~6 source modules, ~250-400 lines each. Plus binary and tests.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | PASS | New workspace crate `swink-agent-rpc`. Self-contained, independently compilable, independently testable. Depends only on `swink-agent` public APIs. The daemon binary is a thin CLI wrapper, not a service framework. |
| II. Test-Driven Development | PASS | Unit tests for JSON-RPC peer (message round-trip, request/response correlation, disconnect handling). Integration tests for full server/client lifecycle. |
| III. Efficiency & Performance | PASS | Zero-copy where possible — messages serialized directly to the write half. Tokio tasks for reader/writer run independently. No unnecessary allocations on the event streaming path. |
| IV. Leverage the Ecosystem | PASS | Uses `tokio` for async I/O, `serde_json` for serialization, `nix` for peer credentials — all well-maintained ecosystem crates. No hand-rolled JSON-RPC framing when standard patterns suffice. |
| V. Provider Agnosticism | PASS | Zero provider-specific code. The server wraps `Agent` which uses `StreamFn` — any provider works through the same socket interface. |
| VI. Safety & Correctness | PASS | `#[forbid(unsafe_code)]`. Peer-credential defense-in-depth. Socket permissions 0600. Single-session enforcement. Malformed messages logged and dropped, not panicked on. |

**Crate count**: Adding a 17th workspace member (constitution says 11 but the workspace already has 16 — evolve, auth, artifacts, mcp, macros were added by prior specs).

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| 17th workspace crate | JSON-RPC transport has distinct dependencies (nix, clap for binary) and a distinct concern (wire protocol, socket I/O, session management) orthogonal to the agent loop. | Embedding in the core crate would pull nix/clap into core. Embedding in the TUI crate would couple remote transport to terminal UI. The crate boundary matches the dependency boundary. |

## Project Structure

### Documentation (this feature)

```text
specs/045-agent-rpc-service/
├── plan.md              # This file
├── spec.md              # Feature specification
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/
│   └── public-api.md    # Phase 1 output
└── checklists/
    └── requirements.md  # Spec quality checklist
```

### Source Code (repository root)

```text
rpc/
├── Cargo.toml
├── src/
│   ├── lib.rs              # Re-exports (AgentServer, AgentClient)
│   ├── dto.rs              # Wire DTOs, From/Into bridges, method constants
│   ├── server.rs           # AgentServer — Unix accept loop, session handler
│   ├── client.rs           # AgentClient — connect + prompt_text API
│   ├── jsonrpc/
│   │   ├── mod.rs          # Re-exports
│   │   ├── message.rs      # RawMessage, RpcError, RequestId
│   │   └── peer.rs         # JsonRpcPeer, PeerSender — reader/writer tasks
│   └── bin/
│       └── swink_agentd.rs # swink-agentd daemon binary
└── tests/
    ├── peer.rs             # JSON-RPC peer unit tests
    └── end_to_end.rs       # Full server/client integration tests
```

**Structure Decision**: Single library crate with a binary target. The `jsonrpc/` submodule isolates the transport-agnostic peer from the agent-specific server/client. Feature gates (`client`, `server`, `cli`) control what compiles.
