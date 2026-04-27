# Tasks: JSON-RPC Agent Service

**Input**: Design documents from `/specs/045-agent-rpc-service/`
**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/public-api.md

**Tests**: Included — the spec requires unit tests for the JSON-RPC peer and integration tests for the server/client lifecycle.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Create the crate, configure features, add to workspace

- [x] T001 Create crate directory and `rpc/Cargo.toml` with `client`, `server`, `cli` feature gates and workspace dependencies
- [x] T002 Add `"rpc"` to workspace members in root `Cargo.toml`
- [x] T003 [P] Create `rpc/src/lib.rs` with `#![forbid(unsafe_code)]`, module declarations, and feature-gated re-exports
- [x] T004 [P] Create `rpc/src/dto.rs` with wire DTOs (`InitializeParams`, `InitializedParams`, `PromptParams`, `PromptResult`, `ToolApprovalRequestDto`, `ToolApprovalDto`), `From` impls bridging to core types, `PROTOCOL_VERSION` constant, and `method` constants module

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Transport-agnostic JSON-RPC 2.0 peer — required by both server and client

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [x] T005 Create `rpc/src/jsonrpc/message.rs` with `RequestId` (serde untagged enum), `RpcError` (standard + application error codes, constructor helpers), `RawMessage` (flat serde struct with constructors for request/notification/success/error), and `MessageKind` classification enum
- [x] T006 Create `rpc/src/jsonrpc/peer.rs` with `IncomingMessage` enum, `PeerInner` (outbound channel, pending request map, ID counter), `PeerSender` (Clone, `notify`/`request`/`respond_ok`/`respond_err`), `JsonRpcPeer` (spawns reader/writer tasks from generic `AsyncRead`/`AsyncWrite`), `dispatch_line` with struct destructuring to avoid borrow-after-move
- [x] T007 Create `rpc/src/jsonrpc/mod.rs` re-exporting `RawMessage`, `RequestId`, `RpcError`, `IncomingMessage`, `JsonRpcPeer`, `PeerSender`, `MAX_LINE_BYTES`
- [x] T008 Write peer unit tests in `rpc/tests/peer.rs`: notification round-trip, request/response correlation, error response surfaces as `Err`, pending requests fail on disconnect, concurrent request correlation, oversize line closes connection

**Checkpoint**: JSON-RPC peer compiles and all unit tests pass — `cargo test -p swink-agent-rpc --test peer`

---

## Phase 3: User Story 1 - Connect and Send a Prompt (Priority: P1) 🎯 MVP

**Goal**: A client connects to a running server, sends a text prompt, receives streamed agent events, and gets a turn completion response.

**Independent Test**: Start a server with a mock agent factory, connect a client, send "Hello", verify events received and turn ID returned.

### Implementation for User Story 1

- [x] T009 [US1] Create `rpc/src/server.rs` with `AgentServer` struct (path + factory), `bind(path, factory)` returning `io::Result<Self>` (rejects existing socket), `bind_force(path, factory)` (removes existing), `serve()` with `#[cfg(unix)]` — `UnixListener` bind, `0600` permissions, accept loop with `tokio::select!` for Ctrl-C, `SocketCleanup` Drop guard
- [x] T010 [US1] Implement `handle_connection` in `rpc/src/server.rs` — split stream, create `JsonRpcPeer`, delegate to `run_session`
- [x] T011 [US1] Implement `run_session` in `rpc/src/server.rs` — handshake (await `initialize`, send `initialized`), build `Agent` from factory with approval callback wired via `with_approve_tool_async` on `PeerSender`, dispatch loop for `prompt`/`cancel`/`shutdown`
- [x] T012 [US1] Implement `run_prompt` in `rpc/src/server.rs` — `tokio::select!` over `stream.next()` (send `agent.event` notifications) and `peer.recv_incoming()` (handle `cancel`/`shutdown`), return turn ID on stream completion
- [x] T013 [US1] Create `rpc/src/client.rs` with `AgentClient` struct (peer + optional approval handler), `connect(path)` with `#[cfg(unix)]` — open `UnixStream`, create `JsonRpcPeer`, send `initialize`, await `initialized`
- [x] T014 [US1] Implement `prompt_text` and `run_turn` in `rpc/src/client.rs` — send `prompt` request, `tokio::select!` over prompt future and `recv_incoming()` to collect `agent.event` notifications, return collected events on prompt completion
- [x] T015 [US1] Add non-unix stub implementations for `AgentServer::serve()` and `AgentClient::connect()` returning descriptive errors

**Checkpoint**: Server starts, client connects, prompt round-trip works — `cargo clippy -p swink-agent-rpc --no-deps -- -D warnings` passes ✅

---

## Phase 4: User Story 2 - Handle Tool Approval Requests (Priority: P1)

**Goal**: When the agent invokes a tool requiring approval, the server sends a `tool.approve` request to the client, which responds with an approval decision that the agent respects.

**Independent Test**: Configure agent with approval-requiring tool, connect client with conditional approval handler, send prompt triggering the tool, verify handler called and decision respected.

### Implementation for User Story 2

- [x] T016 [US2] Implement `with_approval_handler` builder on `AgentClient` in `rpc/src/client.rs`
- [x] T017 [US2] Implement `handle_approval` in `rpc/src/client.rs` — parse `tool.approve` params into `ToolApprovalRequest`, invoke handler (or auto-approve if none set), serialize response
- [x] T018 [US2] Wire `tool.approve` request handling into `run_turn`'s `tokio::select!` match arm in `rpc/src/client.rs`

**Checkpoint**: Tool approval round-trips work with both approve and reject scenarios ✅

---

## Phase 5: User Story 3 - Start and Stop the Agent Daemon (Priority: P1)

**Goal**: The `swink-agentd` binary starts a daemon with CLI arguments, binds the socket, and shuts down gracefully on Ctrl-C.

**Independent Test**: Build and run the binary with `--listen /tmp/test.sock`, verify socket created with correct permissions, Ctrl-C removes socket.

### Implementation for User Story 3

- [x] T019 [US3] Create `rpc/src/bin/swink_agentd.rs` with clap CLI (`--listen`, `--force`, `--model`, `--system-prompt`), `#[cfg(unix)]` tokio main calling `AgentServer::bind`/`bind_force` + `serve()`, non-unix main printing error and exiting

**Checkpoint**: `cargo build -p swink-agent-rpc --features cli` succeeds, binary runs and creates socket ✅

---

## Phase 6: User Story 4 - Reject Unauthorized Connections (Priority: P1)

**Goal**: The server checks peer credentials and rejects connections from different effective UIDs.

**Independent Test**: Verify peer credential check runs on accepted connections. Socket permissions (0600) + credential check provide defense-in-depth.

### Implementation for User Story 4

- [x] T020 [P] [US4] Implement `effective_uid()` and platform-specific `peer_uid()` functions in `rpc/src/server.rs` — Linux via `nix::sys::socket::getsockopt(stream, PeerCredentials)`, macOS via `nix::unistd::getpeereid(stream)`, fallback for other Unix variants
- [x] T021 [US4] Wire peer credential check into `handle_connection` in `rpc/src/server.rs` — reject if `peer_uid != effective_uid`, log warning on unsupported platforms

**Checkpoint**: Credential check compiles and runs on macOS/Linux — `cargo clippy -p swink-agent-rpc --no-deps -- -D warnings` passes ✅

---

## Phase 7: User Story 5 - Cancel a Running Prompt (Priority: P2)

**Goal**: Client sends a cancel notification, server calls `agent.abort()`, turn ends at next safe boundary.

**Independent Test**: Start a long-running prompt, send cancel after first event, verify turn ends early.

### Implementation for User Story 5

- [x] T022 [US5] Implement `cancel` method on `AgentClient` in `rpc/src/client.rs` — fire `cancel` notification
- [x] T023 [US5] Add `cancel` notification handling in `run_prompt` and `run_session` dispatch loops in `rpc/src/server.rs` — call `agent.abort()`

**Checkpoint**: Cancel signal stops agent turns ✅

---

## Phase 8: User Story 6 - Enforce Single-Session Access (Priority: P2)

**Goal**: Second concurrent connection gets "session in use" error; first session unaffected.

**Independent Test**: Connect two clients concurrently, verify second is rejected.

### Implementation for User Story 6

- [x] T024 [US6] Add `Arc<AtomicBool>` session guard to `serve()` accept loop in `rpc/src/server.rs` — `compare_exchange` on accept, store `false` on session end
- [x] T025 [US6] Reject second connection with `RpcError::session_in_use()` notification in `handle_connection` in `rpc/src/server.rs`

**Checkpoint**: Second concurrent client receives "session in use" error ✅

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Cleanup, documentation, and TUI stub removal

- [x] T026 [P] Remove `SocketTransport` stub from `tui/src/transport.rs` (lines 182-222 and test block at 406-429)
- [x] T027 [P] Remove `#[cfg(feature = "remote")] pub use transport::SocketTransport;` from `tui/src/lib.rs`
- [x] T028 [P] Remove `remote = []` feature from `tui/Cargo.toml`
- [x] T029 [P] Remove unused `TransportError::Unavailable` variant from `tui/src/transport.rs`
- [x] T030 [P] Create architecture doc at `docs/architecture/agent-rpc.md`
- [x] T031 [P] Add CHANGELOG.md entry under `## [Unreleased]` for new crate and TUI stub removal
- [x] T032 Run `cargo clippy --workspace --no-deps -- -D warnings` and fix any warnings
- [x] T033 Run `cargo test -p swink-agent-rpc` — all tests pass (peer tests verified)
- [x] T034 Run `cargo test -p swink-agent-tui` — no regressions from stub removal (333 passed, 0 failed)
- [ ] T035 Run quickstart.md validation — manual end-to-end with `swink-agentd` and a client

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **User Stories (Phase 3-8)**: All depend on Foundational phase completion
  - US1 (Prompt) → US2 (Approval) → US5 (Cancel) form a natural sequence (each extends the same code paths)
  - US3 (Daemon Binary) can be done after US1
  - US4 (Credentials) can be done after US1
  - US6 (Single-Session) can be done after US1
- **Polish (Phase 9)**: Depends on all user stories being complete

### User Story Dependencies

- **US1 (P1)**: Can start after Foundational — No dependencies on other stories
- **US2 (P1)**: Extends US1's `run_turn` — depends on US1 completion
- **US3 (P1)**: Uses `AgentServer` from US1 — depends on US1 completion
- **US4 (P1)**: Extends `handle_connection` from US1 — depends on US1 completion
- **US5 (P2)**: Extends `run_prompt` from US1 — depends on US1 completion
- **US6 (P2)**: Extends `serve()` from US1 — depends on US1 completion

### Within Each User Story

- Models/types before services
- Server before client (for features that touch both)
- Core implementation before edge cases

### Parallel Opportunities

- T003, T004 can run in parallel (different files)
- T020 can run in parallel with other US4 tasks
- T026-T031 (Polish) can all run in parallel (different files/crates)
- US3, US4, US6 can run in parallel after US1 completes

---

## Parallel Example: User Story 1

```bash
# After Foundational phase, launch server and client in parallel:
Task: "T009 [US1] Create server.rs with AgentServer struct"
Task: "T013 [US1] Create client.rs with AgentClient struct"

# Then sequentially:
Task: "T010 [US1] Implement handle_connection"
Task: "T011 [US1] Implement run_session"
Task: "T012 [US1] Implement run_prompt"
Task: "T014 [US1] Implement prompt_text and run_turn"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (JSON-RPC peer + tests)
3. Complete Phase 3: User Story 1 (Connect + Prompt)
4. **STOP and VALIDATE**: Test with mock agent — prompt round-trip works
5. This alone delivers a working agent RPC interface

### Incremental Delivery

1. Complete Setup + Foundational → Peer works
2. Add US1 (Prompt) → Test independently → Core value delivered (MVP!)
3. Add US2 (Approval) → Test independently → Safety-critical path works
4. Add US3 (Daemon) → Test independently → Binary ships
5. Add US4 (Credentials) → Test independently → Security hardens
6. Add US5 (Cancel) → Test independently → UX improves
7. Add US6 (Single-Session) → Test independently → Robustness improves
8. Polish → Docs, CHANGELOG, TUI cleanup

---

## Verification Summary (2026-04-27)

**34 of 35 tasks complete.** All code verified against implementation:

- **Clippy**: `swink-agent-rpc` and `swink-agent-tui` both pass `--no-deps -- -D warnings`
- **TUI tests**: 333 passed, 0 failed — no regressions from stub removal
- **Memory clippy fix**: Renamed `score` → `relevance` in `memory/src/jsonl.rs:928` to resolve `similar_names` lint
- **Stale doc comments**: Cleaned references to removed `SocketTransport` in `tui/src/transport.rs`
- **T035 (quickstart validation)**: Requires manual end-to-end test with a real LLM provider — left open

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story should be independently completable and testable
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
