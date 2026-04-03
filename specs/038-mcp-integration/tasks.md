# Tasks: MCP Integration

**Input**: Design documents from `/specs/038-mcp-integration/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/

**Tests**: Included — the project constitution mandates test-driven development.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Create the `swink-agent-mcp` crate and wire it into the workspace

- [x] T001 Add `mcp` to workspace members in Cargo.toml and create mcp/Cargo.toml with dependencies: swink-agent (path), rmcp (with client + sse features), tokio, serde, serde_json, thiserror, tracing
- [x] T002 Create mcp/src/lib.rs with `#![forbid(unsafe_code)]`, module declarations, and public re-exports per contracts/public-api.md
- [x] T003 [P] Create mcp/src/error.rs with McpError enum: SpawnFailed, ConnectionFailed, ToolNameCollision, ToolCallFailed, ProtocolError — implement Display, Error, and From conversions
- [x] T004 [P] Create mcp/tests/common/mod.rs with mock MCP server helpers: a function that spawns an in-process stdio MCP server advertising configurable tools and returning configurable results

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core types and conversion logic that ALL user stories depend on

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [x] T005 Create mcp/src/config.rs with McpTransport enum (Stdio { command, args, env }, Sse { url, bearer_token }), McpServerConfig struct (name, transport, tool_prefix, tool_filter, requires_approval), and ToolFilter struct (allow, deny) per data-model.md
- [x] T006 Create mcp/src/convert.rs with functions: rmcp Tool → (name: String, description: String, input_schema: Value), rmcp Content → swink-agent ContentBlock (Text→Text, Image→Image, other→Text fallback), rmcp CallToolResult → AgentToolResult
- [x] T007 Add MCP event variants to src/loop_/event.rs in the AgentEvent enum: McpServerConnected { server_name }, McpServerDisconnected { server_name, reason }, McpToolsDiscovered { server_name, tool_count }, McpToolCallStarted { server_name, tool_name }, McpToolCallCompleted { server_name, tool_name, is_error }
- [x] T008 [P] Write tests for convert module in mcp/tests/convert_test.rs: text content conversion, image content conversion, unsupported content fallback, error result conversion, empty content handling
- [x] T009 [P] Write tests for config module in mcp/tests/config_test.rs: McpServerConfig construction, ToolFilter evaluation logic (allow-only, deny-only, both, neither)

**Checkpoint**: Foundation ready — core types, conversions, and events are in place

---

## Phase 3: User Story 1 — Agent Discovers and Uses Tools from an MCP Server (Priority: P1) 🎯 MVP

**Goal**: Connect to a single MCP server via stdio, discover tools, and execute tool calls

**Independent Test**: Start a mock MCP server subprocess, connect, verify tools appear in agent tool list and can be called

### Tests for User Story 1

- [ ] T010 [P] [US1] Write test in mcp/tests/connection_test.rs: connect to mock stdio MCP server, verify connection succeeds and tools are discovered
- [ ] T011 [P] [US1] Write test in mcp/tests/tool_test.rs: create McpTool from discovered tool, verify name(), description(), parameters_schema() return correct values from MCP server
- [ ] T012 [P] [US1] Write test in mcp/tests/tool_test.rs: execute McpTool, verify call is forwarded to MCP server and result is converted to AgentToolResult
- [ ] T013 [P] [US1] Write test in mcp/tests/connection_test.rs: attempt connection to non-existent server, verify graceful error with McpError::SpawnFailed

### Implementation for User Story 1

- [ ] T014 [US1] Create mcp/src/connection.rs with McpConnection struct: holds McpServerConfig, rmcp RunningService, discovered tools Vec, and McpConnectionStatus enum (Connected, Disconnected)
- [ ] T015 [US1] Implement McpConnection::connect() for stdio transport: spawn subprocess via rmcp TokioChildProcessBuilder with configured command, args, and env vars, call peer().list_all_tools(), store discovered tools
- [ ] T016 [US1] Create mcp/src/tool.rs with McpTool struct implementing AgentTool trait: name() returns prefixed name, description() and parameters_schema() from MCP Tool definition, metadata() returns ToolMetadata::with_namespace(server_name), requires_approval() returns config value
- [ ] T017 [US1] Implement McpTool::execute() in mcp/src/tool.rs: construct CallToolRequestParams from params Value, call connection.peer().call_tool(), convert CallToolResult to AgentToolResult via convert module, respect cancellation token via tokio::select!
- [ ] T018 [US1] Create mcp/src/event.rs with helper functions for emitting MCP AgentEvent variants through a provided event dispatcher closure

**Checkpoint**: Single MCP server connection works end-to-end — tools discovered and callable

---

## Phase 4: User Story 2 — Consumer Connects to Multiple MCP Servers (Priority: P1)

**Goal**: Orchestrate connections to multiple MCP servers with name prefixing to avoid collisions

**Independent Test**: Start two mock MCP servers with same tool names, configure different prefixes, verify both tools available with distinct names

### Tests for User Story 2

- [ ] T019 [P] [US2] Write test in mcp/tests/manager_test.rs: connect to two mock servers with prefixes, verify tools are prefixed correctly (prefix_toolname)
- [ ] T020 [P] [US2] Write test in mcp/tests/manager_test.rs: connect to three servers where one fails, verify other two servers' tools are available
- [ ] T021 [P] [US2] Write test in mcp/tests/manager_test.rs: connect two servers without prefixes that have same tool name, verify McpError::ToolNameCollision

### Implementation for User Story 2

- [ ] T022 [US2] Create mcp/src/manager.rs with McpManager struct: holds Vec<McpServerConfig>, Vec<McpConnection>, and flattened Vec<Arc<dyn AgentTool>>
- [ ] T023 [US2] Implement McpManager::new(configs) and McpManager::connect_all() in mcp/src/manager.rs: iterate configs, call McpConnection::connect() for each, collect tools, apply prefixes, detect name collisions across servers, log failures and continue with partial results
- [ ] T024 [US2] Implement McpManager::tools() returning Vec<Arc<dyn AgentTool>> and McpManager::shutdown() for graceful cleanup in mcp/src/manager.rs
- [ ] T025 [US2] Implement tool name collision detection in McpManager::connect_all(): after collecting all tools, check for duplicate names and return McpError::ToolNameCollision with server names

**Checkpoint**: Multi-server orchestration works — tools from multiple servers coexist with prefix namespacing

---

## Phase 5: User Story 3 — MCP Tools Participate in Policy and Approval Gates (Priority: P1)

**Goal**: Verify MCP tools go through the same policy/approval pipeline as native tools

**Independent Test**: Configure agent with deny-list policy blocking an MCP tool name, verify the tool call is rejected

### Tests for User Story 3

- [ ] T026 [P] [US3] Write test in mcp/tests/policy_test.rs: register McpTool on agent with approval function, verify approval gate fires before MCP tool execution
- [ ] T027 [P] [US3] Write test in mcp/tests/policy_test.rs: register McpTool that requires_approval=true, verify requires_approval() returns true on the AgentTool trait

### Implementation for User Story 3

- [ ] T028 [US3] Verify McpTool::requires_approval() in mcp/src/tool.rs returns the value from McpServerConfig.requires_approval (should already be implemented in T016 — add test-only validation if needed)
- [ ] T029 [US3] Verify McpTool produces correct approval_context() in mcp/src/tool.rs: return the tool call params as approval context so policy/approval UI can inspect them

**Checkpoint**: MCP tools are fully governed by existing policy infrastructure — no security bypass

---

## Phase 6: User Story 4 — Consumer Filters Which MCP Tools Are Exposed (Priority: P2)

**Goal**: Allow-list and deny-list filtering of tools per MCP server connection

**Independent Test**: Configure MCP server with allow-list of 2 tools out of 10, verify only those 2 are registered

### Tests for User Story 4

- [ ] T030 [P] [US4] Write test in mcp/tests/filter_test.rs: mock server with 5 tools, allow-list of 2, verify only 2 tools returned
- [ ] T031 [P] [US4] Write test in mcp/tests/filter_test.rs: mock server with 5 tools, deny-list of 1, verify 4 tools returned
- [ ] T032 [P] [US4] Write test in mcp/tests/filter_test.rs: mock server with both allow and deny lists, verify allow applied first then deny

### Implementation for User Story 4

- [ ] T033 [US4] Implement ToolFilter::apply() method in mcp/src/config.rs: takes Vec<Tool> and returns filtered Vec<Tool> — if allow is Some, keep only matching names; then if deny is Some, remove matching names
- [ ] T034 [US4] Integrate ToolFilter::apply() into McpConnection::connect() in mcp/src/connection.rs: after tool discovery, apply filter before storing tools

**Checkpoint**: Tool filtering works — consumers can control which MCP tools are exposed

---

## Phase 7: User Story 5 — Agent Connects to Remote MCP Servers via SSE (Priority: P2)

**Goal**: Connect to remote MCP servers over HTTP/SSE with optional bearer token authentication

**Independent Test**: Run local HTTP MCP server, connect via SSE transport, verify tool discovery and execution work

### Tests for User Story 5

- [ ] T035 [P] [US5] Write test in mcp/tests/connection_test.rs: connect to mock SSE MCP server, verify tool discovery works over HTTP
- [ ] T036 [P] [US5] Write test in mcp/tests/connection_test.rs: connect to SSE server with bearer token configured, verify Authorization header is sent

### Implementation for User Story 5

- [ ] T037 [US5] Implement McpConnection::connect() for SSE transport in mcp/src/connection.rs: use rmcp SSE client transport with configured URL, add bearer token as Authorization header if configured
- [ ] T038 [US5] Implement SSE reconnection logic in mcp/src/connection.rs: on connection drop, attempt reconnect with backoff, update McpConnectionStatus, emit McpServerDisconnected event on failure

**Checkpoint**: SSE transport works — remote MCP servers accessible over HTTP with auth

---

## Phase 8: User Story 6 — MCP Server Lifecycle Is Managed Automatically (Priority: P2)

**Goal**: Subprocess spawning on start, termination on drop, crash detection

**Independent Test**: Start agent with MCP subprocess, drop agent, verify subprocess is terminated

### Tests for User Story 6

- [ ] T039 [P] [US6] Write test in mcp/tests/lifecycle_test.rs: connect to stdio MCP server, drop McpManager, verify subprocess is terminated (check process no longer running)
- [ ] T040 [P] [US6] Write test in mcp/tests/lifecycle_test.rs: connect to stdio MCP server, kill subprocess externally, verify McpServerDisconnected event is emitted and tools are marked unavailable

### Implementation for User Story 6

- [ ] T041 [US6] Implement Drop for McpManager in mcp/src/manager.rs: iterate connections and drop rmcp sessions (rmcp's ChildWithCleanup handles subprocess termination)
- [ ] T042 [US6] Implement crash detection in mcp/src/connection.rs: spawn a background tokio task that monitors the rmcp session health, on failure update status to Disconnected and emit McpServerDisconnected event
- [ ] T043 [US6] Implement McpTool::execute() error path in mcp/src/tool.rs: if connection status is Disconnected, return AgentToolResult::error() immediately without attempting the call

**Checkpoint**: Full lifecycle management — no zombie processes, crash detection, graceful degradation

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Final integration, documentation, and validation

- [ ] T044 [P] Update mcp/src/lib.rs public re-exports to include all public types: McpManager, McpServerConfig, McpTransport, ToolFilter, McpError, McpTool
- [ ] T045 [P] Add mcp crate to root Cargo.toml workspace dependencies if needed by other crates (e.g., integration tests)
- [ ] T046 Verify cargo test --workspace passes with mcp crate included
- [ ] T047 Verify cargo clippy --workspace -- -D warnings passes with zero warnings
- [ ] T048 Verify cargo build --workspace compiles without mcp feature having any effect on non-mcp crates (feature isolation)
- [ ] T049 Run quickstart.md validation: verify the code examples in specs/038-mcp-integration/quickstart.md compile and work correctly against the implementation

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **US1 (Phase 3)**: Depends on Foundational — MVP, must complete first
- **US2 (Phase 4)**: Depends on US1 (builds McpManager on top of McpConnection)
- **US3 (Phase 5)**: Depends on US1 (needs McpTool to test policy integration)
- **US4 (Phase 6)**: Depends on US1 (integrates filtering into McpConnection)
- **US5 (Phase 7)**: Depends on US1 (adds SSE transport to existing McpConnection)
- **US6 (Phase 8)**: Depends on US2 (lifecycle managed at McpManager level)
- **Polish (Phase 9)**: Depends on all user stories

### User Story Dependencies

- **US1 (P1)**: Foundation only — no story dependencies
- **US2 (P1)**: Depends on US1 (McpManager wraps McpConnection)
- **US3 (P1)**: Depends on US1 (McpTool must exist to test policies)
- **US4 (P2)**: Depends on US1 (filter applied in McpConnection)
- **US5 (P2)**: Depends on US1 (SSE is alternate transport in McpConnection)
- **US6 (P2)**: Depends on US2 (lifecycle managed by McpManager)

### Within Each User Story

- Tests MUST be written and FAIL before implementation
- Types/structs before methods
- Connection logic before tool logic
- Core implementation before integration

### Parallel Opportunities

- T003/T004 can run in parallel (error.rs and test helpers)
- T005/T006/T007 can run in parallel within Foundational (different files)
- T008/T009 can run in parallel (different test files)
- All tests within a story marked [P] can run in parallel
- US3, US4, and US5 can run in parallel after US1 completes (different concerns)

---

## Parallel Example: User Story 1

```bash
# Launch all US1 tests together:
Task: "Write test in mcp/tests/connection_test.rs: connect to mock stdio MCP server"
Task: "Write test in mcp/tests/tool_test.rs: create McpTool, verify trait methods"
Task: "Write test in mcp/tests/tool_test.rs: execute McpTool, verify forwarding"
Task: "Write test in mcp/tests/connection_test.rs: non-existent server error"

# After tests, launch connection + tool implementation:
# T014 (connection.rs) must complete before T015-T017
# T016 (tool struct) can start after T014
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (T001–T004)
2. Complete Phase 2: Foundational (T005–T009)
3. Complete Phase 3: User Story 1 (T010–T018)
4. **STOP and VALIDATE**: Connect to a real MCP server, discover tools, call one
5. This alone enables the entire MCP tool ecosystem for Swink agents

### Incremental Delivery

1. Setup + Foundational → Crate exists, types compile
2. US1 → Single server works → MVP!
3. US2 → Multi-server with prefixes → Production-ready
4. US3 → Policy integration verified → Security validated
5. US4 → Tool filtering → Prompt size control
6. US5 → SSE transport → Remote servers
7. US6 → Lifecycle management → Operational robustness
8. Polish → CI-clean, documented

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story should be independently completable and testable
- Verify tests fail before implementing
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
