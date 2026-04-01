# Tasks: Session Key-Value State Store

**Input**: Design documents from `/specs/034-session-state-store/`
**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/public-api.md

**Tests**: Included per constitution principle II (Test-Driven Development — NON-NEGOTIABLE).

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Phase 1: Setup

**Purpose**: Create the new source file for core types

- [ ] T001 Create `src/state.rs` with module declaration and `#[forbid(unsafe_code)]` at crate root maintained

---

## Phase 2: Foundational (Core Types)

**Purpose**: Implement `SessionState` and `StateDelta` — BLOCKS all user stories

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

### Tests

- [ ] T002 [P] Write tests for `StateDelta`: empty default, `is_empty()`, `len()`, serialize/deserialize roundtrip in `src/state.rs`
- [ ] T003 [P] Write tests for `SessionState::get`/`set`/`remove`/`contains`/`keys`/`len`/`is_empty`/`clear` in `src/state.rs`
- [ ] T004 [P] Write tests for `SessionState::get_raw` returning `Option<&Value>` without deserialization in `src/state.rs`
- [ ] T005 [P] Write tests for delta collapse semantics: set+set→last, set+remove→None, remove+set→Some, clear→all None in `src/state.rs`
- [ ] T006 [P] Write tests for `delta()`/`flush_delta()`: pending changes returned, delta reset after flush, empty flush returns empty in `src/state.rs`
- [ ] T007 [P] Write tests for `SessionState::with_data` pre-seeding: data populated, delta empty (baseline semantics) in `src/state.rs`
- [ ] T008 [P] Write tests for `SessionState::snapshot`/`restore_from_snapshot` roundtrip in `src/state.rs`
- [ ] T009 [P] Write test for typed get with mismatched type returns `None` without corrupting stored value in `src/state.rs`

### Implementation

- [ ] T010 Implement `StateDelta` struct with `changes: HashMap<String, Option<Value>>`, `is_empty()`, `len()`, derive `Default`, `Clone`, `Debug`, `Serialize`, `Deserialize` in `src/state.rs`
- [ ] T011 Implement `SessionState` struct with `data: HashMap<String, Value>` and `delta: StateDelta` (skip delta on serialize), derive `Default`, `Clone`, `Debug`, `Serialize`, `Deserialize` in `src/state.rs`
- [ ] T012 Implement `SessionState::new()` and `SessionState::with_data(data)` (pre-seed without delta) in `src/state.rs`
- [ ] T013 Implement `SessionState::get<T>`, `get_raw`, `set<T>`, `remove`, `contains`, `keys`, `len`, `is_empty`, `clear` with delta tracking in `src/state.rs`
- [ ] T014 Implement delta collapse logic in `set` and `remove`: last-writer-wins within a delta window in `src/state.rs`
- [ ] T015 Implement `delta()` and `flush_delta()` on `SessionState` in `src/state.rs`
- [ ] T016 Implement `snapshot()` and `restore_from_snapshot()` on `SessionState` in `src/state.rs`
- [ ] T017 Add `pub use state::{SessionState, StateDelta};` to `src/lib.rs`
- [ ] T018 Verify all Phase 2 tests pass with `cargo test -p swink-agent`

**Checkpoint**: Core types complete — `SessionState` and `StateDelta` are usable in isolation

---

## Phase 3: User Story 1 — Tool Stores Structured Data Across Turns (Priority: P1) 🎯 MVP

**Goal**: Tools can read/write key-value state during execution, persisting data across turns

**Independent Test**: Create a mock tool that sets a state key, run a multi-turn agent, verify subsequent tool reads the value

### Tests

- [ ] T019 [P] [US1] Write test for `Agent::session_state()` accessor returns `Arc<RwLock<SessionState>>` in `src/agent.rs`
- [ ] T020 [P] [US1] Write test that `AgentTool::execute` receives state and can read/write it in `tests/common/mod.rs`
- [ ] T021 [US1] Write integration test: mock tool sets state key on turn 1, second tool reads it on turn 2, value persists in `tests/state_tests.rs`

### Implementation

- [ ] T022 [US1] Add `session_state: Arc<RwLock<SessionState>>` field to `Agent` struct in `src/agent.rs`
- [ ] T023 [US1] Add `session_state()` method returning `&Arc<RwLock<SessionState>>` to `Agent` impl in `src/agent.rs`
- [ ] T024 [US1] Initialize `session_state` from `AgentOptions` in `Agent::new()` in `src/agent.rs`
- [ ] T025 [US1] Add `session_state: Option<SessionState>` field to `AgentOptions` in `src/agent_options.rs`
- [ ] T026 [US1] Add `state: Arc<RwLock<SessionState>>` parameter to `AgentTool::execute` trait method in `src/tool.rs`
- [ ] T027 [US1] Update `BashTool::execute` to accept state parameter (unused, pass-through) in `src/builtin_tools/bash.rs`
- [ ] T028 [P] [US1] Update `ReadFileTool::execute` to accept state parameter in `src/builtin_tools/read_file.rs`
- [ ] T029 [P] [US1] Update `WriteFileTool::execute` to accept state parameter in `src/builtin_tools/write_file.rs`
- [ ] T030 [US1] Update `MockTool::execute` in shared test helpers to accept and optionally use state in `tests/common/mod.rs`
- [ ] T031 [US1] Update tool dispatch in agent loop to pass `Arc<RwLock<SessionState>>` to `tool.execute()` in `src/loop_/mod.rs`
- [ ] T032 [US1] Thread `session_state` from `Agent` through `AgentLoopConfig` to the loop in `src/loop_/mod.rs`
- [ ] T033 [US1] Add `session_state: Arc<RwLock<SessionState>>` field to `AgentLoopConfig` in `src/loop_/mod.rs`
- [ ] T034 [US1] Update any other `AgentTool` implementations in the workspace (eval, TUI, integration tests) to accept state parameter
- [ ] T035 [US1] Verify all US1 tests pass with `cargo test --workspace`

**Checkpoint**: Tools can read/write state — User Story 1 is independently functional

---

## Phase 4: User Story 2 — State Survives Session Save and Restore (Priority: P1)

**Goal**: State persists across session save/load cycles via SessionStore

**Independent Test**: Set state, save session, load session in new agent, verify state restored

### Tests

- [ ] T036 [P] [US2] Write test for `SessionStore::save_state`/`load_state` default impls (no-op, returns None) in `memory/src/store.rs`
- [ ] T037 [P] [US2] Write test for `JsonlSessionStore` state persistence: save state, load state, verify roundtrip in `memory/src/jsonl.rs`
- [ ] T038 [P] [US2] Write test for backward compat: load pre-034 session (no state line) returns empty state in `memory/src/jsonl.rs`
- [ ] T039 [P] [US2] Write test for state with nested JSON values survives roundtrip in `memory/src/jsonl.rs`
- [ ] T040 [P] [US2] Write test for `Checkpoint` with `state: Some(value)` serialization/deserialization roundtrip in `src/checkpoint.rs`
- [ ] T041 [P] [US2] Write test for `Checkpoint` with `state: None` (backward compat, missing field deserializes to None) in `src/checkpoint.rs`
- [ ] T042 [P] [US2] Write test for `LoopCheckpoint` state field roundtrip in `src/checkpoint.rs`

### Implementation

- [ ] T043 [US2] Add `save_state(&self, id: &str, state: &Value) -> io::Result<()>` default method to `SessionStore` trait in `memory/src/store.rs`
- [ ] T044 [US2] Add `load_state(&self, id: &str) -> io::Result<Option<Value>>` default method to `SessionStore` trait in `memory/src/store.rs`
- [ ] T045 [US2] Implement `save_state` in `JsonlSessionStore`: write `{"_state": true, "data": ...}` line, replace if exists in `memory/src/jsonl.rs`
- [ ] T046 [US2] Implement `load_state` in `JsonlSessionStore`: scan for `_state` line, parse data field, return None if absent in `memory/src/jsonl.rs`
- [ ] T047 [US2] Add `#[serde(default)] pub state: Option<Value>` field to `Checkpoint` in `src/checkpoint.rs`
- [ ] T048 [US2] Add `#[serde(default)] pub state: Option<Value>` field to `LoopCheckpoint` in `src/checkpoint.rs`
- [ ] T049 [US2] Update `Agent::save_checkpoint` to include `state.snapshot()` in checkpoint in `src/agent.rs`
- [ ] T050 [US2] Update `Agent::restore_from_checkpoint` to restore state from checkpoint in `src/agent.rs`
- [ ] T051 [US2] Update `Agent::pause` to include state snapshot in `LoopCheckpoint` in `src/agent.rs`
- [ ] T052 [US2] Update `Agent::resume_stream` to restore state from `LoopCheckpoint` in `src/agent.rs`
- [ ] T053 [US2] Verify all US2 tests pass with `cargo test --workspace`

**Checkpoint**: State survives session save/load and checkpoint cycles — User Story 2 is independently functional

---

## Phase 5: User Story 3 — Concurrent Tool Executions Access State Safely (Priority: P1)

**Goal**: Multiple tools can read/write state concurrently without data races

**Independent Test**: Run agent with concurrent tool execution and two tools that both read/write state, verify no panics and all writes reflected

### Tests

- [ ] T054 [P] [US3] Write test: two tasks concurrently read the same key, both get correct value in `tests/state_tests.rs`
- [ ] T055 [P] [US3] Write test: two tasks concurrently write different keys, both writes reflected in `tests/state_tests.rs`
- [ ] T056 [P] [US3] Write test: two tasks concurrently write same key, last-writer-wins, no panic in `tests/state_tests.rs`
- [ ] T057 [US3] Write test: poisoned lock recovery — simulate panic during write, subsequent access recovers via `into_inner()` in `tests/state_tests.rs`

### Implementation

- [ ] T058 [US3] Verify `Arc<RwLock<SessionState>>` is `Send + Sync` (compile-time assertion) in `src/state.rs`
- [ ] T059 [US3] Ensure all state access in tool dispatch uses proper read/write lock patterns with `PoisonError::into_inner()` recovery in `src/loop_/mod.rs`
- [ ] T060 [US3] Verify all US3 tests pass with `cargo test --workspace`

**Checkpoint**: Concurrent access is safe — User Story 3 is independently functional

---

## Phase 6: User Story 4 — Delta Tracking for Efficient Persistence (Priority: P2)

**Goal**: State changes are tracked per-turn, flushed as deltas, and emitted as events

**Independent Test**: Set keys, flush delta, verify delta contains only changes since last flush; verify StateChanged event emitted

### Tests

- [ ] T061 [P] [US4] Write test for `AgentEvent::StateChanged` variant existence and `delta` field access in `src/loop_/mod.rs`
- [ ] T062 [P] [US4] Write test for `TurnSnapshot.state_delta` field: `Some(delta)` when changes, `None` when no changes in `src/types.rs`
- [ ] T063 [US4] Write integration test: subscribe to events, run agent with state-mutating tool, verify `StateChanged` emitted before `TurnEnd` in `tests/state_tests.rs`
- [ ] T064 [US4] Write integration test: run agent with no state mutations, verify `StateChanged` is NOT emitted (suppressed for empty delta) in `tests/state_tests.rs`

### Implementation

- [ ] T065 [US4] Add `StateChanged { delta: StateDelta }` variant to `AgentEvent` enum in `src/loop_/mod.rs`
- [ ] T066 [US4] Add `state_delta: Option<StateDelta>` field to `TurnSnapshot` in `src/types.rs`
- [ ] T067 [US4] Add delta flush logic at turn end in loop: after PostTurn policies, call `flush_delta()`, check non-empty in `src/loop_/mod.rs`
- [ ] T068 [US4] Emit `AgentEvent::StateChanged { delta }` when flushed delta is non-empty, immediately before `TurnEnd` emission in `src/loop_/mod.rs`
- [ ] T069 [US4] Include flushed `StateDelta` as `state_delta` field in `TurnSnapshot` within `TurnEnd` event in `src/loop_/mod.rs`
- [ ] T070 [US4] Verify all US4 tests pass with `cargo test --workspace`

**Checkpoint**: Delta tracking and event emission work — User Story 4 is independently functional

---

## Phase 7: User Story 5 — Policies Can Read State for Decisions (Priority: P2)

**Goal**: Policies receive read-only state access via PolicyContext

**Independent Test**: Implement a custom policy that reads a state key and returns Stop/Continue based on value

### Tests

- [ ] T071 [P] [US5] Write test for `PolicyContext` containing `state: &SessionState` field in `src/policy.rs`
- [ ] T072 [US5] Write test: custom PreTurnPolicy reads state key, returns Stop when key absent in `tests/state_tests.rs`
- [ ] T073 [US5] Write test: custom PreTurnPolicy reads state key, returns Continue when key present and valid in `tests/state_tests.rs`

### Implementation

- [ ] T074 [US5] Add `state: &'a SessionState` field to `PolicyContext<'a>` in `src/policy.rs`
- [ ] T075 [US5] Update all policy slot runners to acquire read lock on state and pass `&SessionState` to `PolicyContext` in `src/loop_/mod.rs`
- [ ] T076 [US5] Update `PolicyContext` construction sites in loop to include state reference in `src/loop_/mod.rs`
- [ ] T077 [US5] Update policy implementations in `swink-agent-policies` crate to accept new `PolicyContext` shape (compile fix) in `policies/src/*.rs`
- [ ] T078 [US5] Verify all US5 tests pass with `cargo test --workspace`

**Checkpoint**: Policies can read state — User Story 5 is independently functional

---

## Phase 8: User Story 6 — Agent Owner Reads and Pre-Seeds State (Priority: P3)

**Goal**: Library consumers can pre-seed state before first turn and read it after conversation ends

**Independent Test**: Pre-seed state via builder, run agent, verify tool sees pre-seeded value; read state after run completes

### Tests

- [ ] T079 [P] [US6] Write test for `AgentOptions::with_initial_state()` builder method in `src/agent_options.rs`
- [ ] T080 [P] [US6] Write test for `AgentOptions::with_state_entry()` builder method (single key-value) in `src/agent_options.rs`
- [ ] T081 [US6] Write test: pre-seeded state has empty delta (baseline semantics confirmed) in `src/agent_options.rs`
- [ ] T082 [US6] Write integration test: pre-seed state, run agent, tool reads pre-seeded value in `tests/state_tests.rs`
- [ ] T083 [US6] Write test: read state from agent after conversation ends, keys accessible in `tests/state_tests.rs`

### Implementation

- [ ] T084 [US6] Implement `with_initial_state(state: SessionState) -> Self` on `AgentOptions` in `src/agent_options.rs`
- [ ] T085 [US6] Implement `with_state_entry(key: impl Into<String>, value: impl Serialize) -> Self` on `AgentOptions` in `src/agent_options.rs`
- [ ] T086 [US6] Verify `Agent::new()` correctly initializes from pre-seeded `AgentOptions.session_state` in `src/agent.rs`
- [ ] T087 [US6] Verify all US6 tests pass with `cargo test --workspace`

**Checkpoint**: Pre-seeding and post-run extraction work — User Story 6 is independently functional

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Integration validation, downstream updates, workspace-wide verification

- [ ] T088 [P] Update `Agent::state()` docs (if any) to clarify relationship with `session_state()` in `src/agent.rs`
- [ ] T089 [P] Update re-exports in `src/lib.rs` if any types were missed during Phase 2
- [ ] T090 Run `cargo clippy --workspace -- -D warnings` and fix any warnings
- [ ] T091 Run `cargo test --workspace` to verify all tests pass end-to-end
- [ ] T092 Run `cargo test -p swink-agent --no-default-features` to verify builtin-tools disabled still compiles
- [ ] T093 Run quickstart.md scenarios manually to validate examples are accurate

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Phase 1 — BLOCKS all user stories
- **US1 (Phase 3)**: Depends on Phase 2 — agent + tool integration
- **US2 (Phase 4)**: Depends on Phase 2 — can run parallel with US1
- **US3 (Phase 5)**: Depends on US1 (needs tool state access wired up)
- **US4 (Phase 6)**: Depends on US1 (needs state in loop)
- **US5 (Phase 7)**: Depends on Phase 2 — can run parallel with US1
- **US6 (Phase 8)**: Depends on US1 (needs Agent.session_state field)
- **Polish (Phase 9)**: Depends on all user stories being complete

### User Story Dependencies

- **US1 (P1)**: Depends on Foundational only — no other story dependencies
- **US2 (P1)**: Depends on Foundational only — independent of US1 (persistence is separate concern)
- **US3 (P1)**: Depends on US1 (concurrent access requires tool dispatch wiring from US1)
- **US4 (P2)**: Depends on US1 (delta flush requires state in loop from US1)
- **US5 (P2)**: Depends on Foundational only — independent of US1 (policy reads from state reference, not tool wiring)
- **US6 (P3)**: Depends on US1 (pre-seed requires Agent.session_state field from US1)

### Within Each User Story

- Tests MUST be written and FAIL before implementation
- Types before integration
- Core implementation before cross-cutting updates
- Story complete before dependent stories start

### Parallel Opportunities

- Phase 2 tests (T002–T009) can all run in parallel
- US1 builtin tool updates (T027–T029) can run in parallel
- US2 tests (T036–T042) can all run in parallel
- US2 and US5 can run in parallel with US1 (different files, no blocking deps on US1 implementation)
- US3, US4, US6 must wait for US1 completion

---

## Parallel Example: User Story 1

```bash
# After Phase 2 completes, launch US1 tests in parallel:
Task: T019 "Test Agent::session_state() accessor"
Task: T020 "Test AgentTool::execute receives state"

# Launch builtin tool updates in parallel:
Task: T028 "Update ReadFileTool::execute for state param"
Task: T029 "Update WriteFileTool::execute for state param"
```

## Parallel Example: User Story 2

```bash
# US2 tests can all run in parallel:
Task: T036 "Test SessionStore default impls"
Task: T037 "Test JsonlSessionStore state persistence"
Task: T038 "Test backward compat (no state line)"
Task: T039 "Test nested JSON roundtrip"
Task: T040 "Test Checkpoint state roundtrip"
Task: T041 "Test Checkpoint backward compat"
Task: T042 "Test LoopCheckpoint state roundtrip"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (core types)
3. Complete Phase 3: US1 (tool state access)
4. **STOP and VALIDATE**: Tools can read/write state across turns
5. This is a usable, shippable increment

### Incremental Delivery

1. Setup + Foundational → Core types ready
2. Add US1 → Tools can use state → **MVP**
3. Add US2 → State persists across sessions → Durability
4. Add US3 → Concurrent safety verified → Production-ready
5. Add US4 → Delta events for subscribers → Observability
6. Add US5 → Policies read state → Advanced use cases
7. Add US6 → Builder pre-seeding → Developer ergonomics
8. Polish → Workspace-wide validation

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story should be independently completable and testable
- Constitution II: Tests written first, must fail before implementation
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
