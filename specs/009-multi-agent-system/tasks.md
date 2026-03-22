# Tasks: Multi-Agent System

**Input**: Design documents from `/specs/009-multi-agent-system/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Core types and Agent integration needed by all user stories

- [x] T001 Add `AgentId` newtype with monotonic `AtomicU64` counter, `Display`, `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`, `Hash` in `src/registry.rs`
- [x] T002 Add `AgentRef` type alias (`Arc<tokio::sync::Mutex<Agent>>`) in `src/registry.rs`
- [x] T003 Integrate `AgentId` into `Agent` struct — assign via `AgentId::next()` in constructor, expose `pub fn id(&self) -> AgentId` in `src/agent.rs`
- [x] T004 [P] Add `AgentStatus` enum (`Running`, `Completed`, `Failed`, `Cancelled`) with `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq` in `src/handle.rs`

**Checkpoint**: Foundation types ready — user story implementation can begin

---

## Phase 2: User Story 1 — Register and Look Up Named Agents (Priority: P1) 🎯 MVP

**Goal**: Thread-safe `AgentRegistry` for registering, looking up, and removing agents by unique name

**Independent Test**: Register multiple agents with unique names and verify lookup by name from different threads

### Implementation for User Story 1

- [x] T005 [US1] Implement `AgentRegistry` struct with `Arc<RwLock<HashMap<String, AgentRef>>>` field in `src/registry.rs`
- [x] T006 [US1] Implement `AgentRegistry::new()` and `Default` trait in `src/registry.rs`
- [x] T007 [US1] Implement `AgentRegistry::register(name, agent) -> AgentRef` with replace-on-duplicate semantics in `src/registry.rs`
- [x] T008 [US1] Implement `AgentRegistry::get(name) -> Option<AgentRef>` with `RwLock` read guard in `src/registry.rs`
- [x] T009 [US1] Implement `AgentRegistry::remove(name) -> Option<AgentRef>` with `RwLock` write guard in `src/registry.rs`
- [x] T010 [P] [US1] Implement inspection methods `names()`, `len()`, `is_empty()` in `src/registry.rs`
- [x] T011 [US1] Add `PoisonError::into_inner()` recovery on all lock acquisitions in `src/registry.rs`
- [x] T012 [US1] Re-export `AgentId`, `AgentRef`, `AgentRegistry` from `src/lib.rs`
- [x] T013 [US1] Add integration tests for registry: register/lookup, non-existent lookup, thread-safe concurrent access, remove in `tests/registry.rs`

**Checkpoint**: AgentRegistry is fully functional and independently testable

---

## Phase 3: User Story 2 — Send Messages Between Agents (Priority: P1)

**Goal**: Asynchronous inter-agent messaging via `AgentMailbox` and `send_to` free function

**Independent Test**: Create two agents, send a message from one to the other via `send_to`, verify the recipient processes it through its steering queue

### Implementation for User Story 2

- [x] T014 [US2] Implement `AgentMailbox` struct with `Arc<Mutex<Vec<AgentMessage>>>` inbox in `src/messaging.rs`
- [x] T015 [US2] Implement `AgentMailbox::new()`, `Default`, `Clone` in `src/messaging.rs`
- [x] T016 [US2] Implement `AgentMailbox::send(message)` — non-blocking push to inbox in `src/messaging.rs`
- [x] T017 [US2] Implement `AgentMailbox::drain() -> Vec<AgentMessage>` using `std::mem::take` in `src/messaging.rs`
- [x] T018 [P] [US2] Implement inspection methods `has_messages()`, `len()`, `is_empty()` in `src/messaging.rs`
- [x] T019 [US2] Add `PoisonError::into_inner()` recovery on all mutex acquisitions in `src/messaging.rs`
- [x] T020 [US2] Implement `send_to(registry, agent_name, message)` async free function — lookup agent, acquire tokio mutex, call `steer()` in `src/messaging.rs`
- [x] T021 [US2] Return `AgentError::Plugin` from `send_to` when agent not found in registry in `src/messaging.rs`
- [x] T022 [US2] Re-export `AgentMailbox`, `send_to` from `src/lib.rs`
- [x] T023 [US2] Add integration tests for messaging: mailbox send/drain, send_to delivery, send_to nonexistent agent error in `tests/messaging.rs`

**Checkpoint**: Inter-agent messaging works independently via mailbox and registry-based send_to

---

## Phase 4: User Story 3 — Invoke an Agent as a Tool (Priority: P2)

**Goal**: `SubAgent` tool wrapper that presents a child agent as a standard `AgentTool` with cancellation propagation

**Independent Test**: Create a parent agent with a SubAgent tool, invoke it, verify the child runs and returns a result; verify cancellation propagates

### Implementation for User Story 3

- [x] T024 [US3] Define `OptionsFactoryFn` and `MapResultFn` type aliases in `src/sub_agent.rs`
- [x] T025 [US3] Implement `SubAgent` struct with name, label, description, schema, requires_approval, options_factory, map_result fields in `src/sub_agent.rs`
- [x] T026 [US3] Implement `SubAgent::new(name, label, description)` with default prompt schema and panic factory in `src/sub_agent.rs`
- [x] T027 [US3] Implement `SubAgent::simple(name, label, description, system_prompt, model, stream_fn)` convenience constructor in `src/sub_agent.rs`
- [x] T028 [P] [US3] Implement builder methods `with_schema()`, `with_requires_approval()`, `with_options()`, `with_map_result()` in `src/sub_agent.rs`
- [x] T029 [US3] Implement `default_map_result` — extract text from last assistant message, handle error stop reason in `src/sub_agent.rs`
- [x] T030 [US3] Implement `AgentTool` trait for `SubAgent` — name, label, description, parameters_schema, requires_approval in `src/sub_agent.rs`
- [x] T031 [US3] Implement `AgentTool::execute()` — construct fresh Agent via factory, run `prompt_text`, propagate cancellation via `tokio::select!` in `src/sub_agent.rs`
- [x] T032 [P] [US3] Implement `Debug` for `SubAgent` and compile-time `Send + Sync` assertion in `src/sub_agent.rs`
- [x] T033 [US3] Re-export `SubAgent` from `src/lib.rs`
- [x] T034 [US3] Add integration tests: SubAgent tool execution, cancellation propagation, custom result mapping, schema validation in `tests/sub_agent.rs`

**Checkpoint**: SubAgent works as a standard tool in any parent agent's tool list

---

## Phase 5: User Story 4 — Supervise Multiple Agents (Priority: P3)

**Goal**: `AgentOrchestrator` for lifecycle management with supervisor policies, parent/child hierarchy, and request/response messaging

**Independent Test**: Create orchestrator, add agents with parent/child hierarchy, spawn, delegate tasks, verify supervision and shutdown

### Implementation for User Story 4

- [x] T035 [P] [US4] Implement `SupervisorAction` enum (`Restart`, `Stop`, `Escalate`) with `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq` in `src/orchestrator.rs`
- [x] T036 [P] [US4] Implement `SupervisorPolicy` trait with `on_agent_error(name, error) -> SupervisorAction` in `src/orchestrator.rs`
- [x] T037 [US4] Implement `DefaultSupervisor` — restart on retryable errors, stop otherwise in `src/orchestrator.rs`
- [x] T038 [P] [US4] Implement `AgentRequest` struct with `messages: Vec<AgentMessage>` and `reply: oneshot::Sender` in `src/orchestrator.rs`
- [x] T039 [US4] Implement `AgentEntry` internal struct with options_factory, parent, children, max_restarts fields in `src/orchestrator.rs`
- [x] T040 [US4] Implement `AgentOrchestrator` struct with entries map, supervisor, channel_buffer, default_max_restarts in `src/orchestrator.rs`
- [x] T041 [US4] Implement `AgentOrchestrator::new()`, `Default`, builder methods `with_supervisor()`, `with_channel_buffer()`, `with_max_restarts()` in `src/orchestrator.rs`
- [x] T042 [US4] Implement `add_agent(name, factory)` — register top-level agent in `src/orchestrator.rs`
- [x] T043 [US4] Implement `add_child(name, parent, factory)` — register child, update parent's children list, panic if parent missing in `src/orchestrator.rs`
- [x] T044 [P] [US4] Implement hierarchy inspection: `parent_of()`, `children_of()`, `names()`, `contains()` in `src/orchestrator.rs`
- [x] T045 [US4] Implement `OrchestratedHandle` struct with name, request_tx, cancellation_token, join_handle, status in `src/orchestrator.rs`
- [x] T046 [US4] Implement `OrchestratedHandle::send_message(text)` and `send_messages(messages)` with oneshot reply in `src/orchestrator.rs`
- [x] T047 [US4] Implement `OrchestratedHandle::await_result()` — drop channel, await join handle in `src/orchestrator.rs`
- [x] T048 [P] [US4] Implement `OrchestratedHandle::cancel()`, `status()`, `is_done()`, `name()`, `Debug` in `src/orchestrator.rs`
- [x] T049 [US4] Implement `run_agent_loop` async function — receive requests, process with agent, handle cancellation via `tokio::select!` in `src/orchestrator.rs`
- [x] T050 [US4] Implement supervisor policy integration in `run_agent_loop` — Restart (with counter), Escalate, Stop actions in `src/orchestrator.rs`
- [x] T051 [US4] Implement `AgentOrchestrator::spawn(name)` — create channel, token, status, launch `run_agent_loop` via `tokio::spawn` in `src/orchestrator.rs`
- [x] T052 [P] [US4] Implement `Debug` for `AgentOrchestrator` in `src/orchestrator.rs`
- [x] T053 [US4] Re-export `AgentOrchestrator`, `OrchestratedHandle`, `AgentRequest`, `SupervisorPolicy`, `SupervisorAction`, `DefaultSupervisor` from `src/lib.rs`
- [x] T054 [US4] Add unit tests in `src/orchestrator.rs`: add_agent/names, contains, parent/child hierarchy, grandchild hierarchy, missing parent panic, spawn unregistered error, supervisor actions, builder methods, custom policy, debug format, default impl

**Checkpoint**: Full orchestration with supervision, hierarchy, and lifecycle management

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Final integration, documentation, and validation

- [x] T055 [P] Verify all public types re-exported from `src/lib.rs` match `contracts/public-api.md`
- [x] T056 Run `cargo build --workspace` and verify zero errors
- [x] T057 Run `cargo test --workspace` and verify all tests pass
- [x] T058 Run `cargo clippy --workspace -- -D warnings` and verify zero warnings
- [x] T059 Run quickstart.md validation — verify all usage examples compile and are accurate

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **User Story 1 — Registry (Phase 2)**: Depends on Phase 1 (AgentId, AgentRef)
- **User Story 2 — Messaging (Phase 3)**: Depends on Phase 2 (AgentRegistry for `send_to`)
- **User Story 3 — SubAgent (Phase 4)**: Depends on Phase 1 only (AgentId for Agent), independent of US1/US2
- **User Story 4 — Orchestrator (Phase 5)**: Depends on Phase 1 (AgentStatus), independent of US1–US3
- **Polish (Phase 6)**: Depends on all user stories being complete

### User Story Dependencies

- **US1 (Registry)**: Can start after Phase 1 — no other story dependencies
- **US2 (Messaging)**: Depends on US1 (AgentRegistry needed for `send_to`)
- **US3 (SubAgent)**: Independent of US1/US2 — can start after Phase 1
- **US4 (Orchestrator)**: Independent of US1–US3 — can start after Phase 1

### Within Each User Story

- Types/structs before methods
- Core methods before convenience methods
- Implementation before re-exports
- Re-exports before integration tests

### Parallel Opportunities

- T001, T002, T004 can run in parallel (different files)
- US3 (SubAgent) and US4 (Orchestrator) can run in parallel with US1/US2
- Within each story, tasks marked [P] can run in parallel
- All integration test files can be written in parallel

---

## Parallel Example: User Story 1 (Registry)

```
# Launch inspection methods in parallel with core methods:
Task T010: "Implement inspection methods names(), len(), is_empty()"
  (parallel with T005–T009, different methods, no conflicts)
```

## Parallel Example: Cross-Story

```
# After Phase 1, launch these in parallel:
Track A: US1 (T005–T013) → US2 (T014–T023)
Track B: US3 (T024–T034)
Track C: US4 (T035–T054)
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (T001–T004)
2. Complete Phase 2: US1 — Registry (T005–T013)
3. **STOP and VALIDATE**: Test registry independently
4. Agents can be registered and looked up by name

### Incremental Delivery

1. Setup + US1 (Registry) → Named agent lookup works (MVP)
2. Add US2 (Messaging) → Agents can communicate by name
3. Add US3 (SubAgent) → Agents can be composed as tools
4. Add US4 (Orchestrator) → Full lifecycle supervision
5. Each story adds value without breaking previous stories

### Parallel Team Strategy

With multiple developers:

1. All complete Phase 1 together
2. Once Phase 1 is done:
   - Developer A: US1 (Registry) → US2 (Messaging)
   - Developer B: US3 (SubAgent)
   - Developer C: US4 (Orchestrator)
3. Stories complete and integrate independently
