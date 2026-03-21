# Tasks: Agent Struct & Public API

**Input**: Design documents from `/specs/005-agent-struct/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/public-api.md

**Tests**: Tests are included as this feature specification references comprehensive test coverage in `tests/agent*.rs` and `tests/handle.rs`.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Path Conventions

- **Single project**: `src/`, `tests/` at repository root (library crate `swink-agent`)

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Project initialization and shared types that all user stories depend on

- [x] T001 Verify workspace dependencies are declared for tokio, tokio-util, futures, serde_json, tracing in root `Cargo.toml`
- [x] T002 [P] Create `src/agent_options.rs` with `AgentOptions` struct and constructors (`new`, `new_simple`, `from_connections`) plus all `with_*()` builder methods
- [x] T003 [P] Create `src/agent_subscriptions.rs` with `ListenerRegistry` struct and `SubscriptionId` type

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core Agent struct shell and shared state types that MUST be complete before ANY user story can be implemented

**CRITICAL**: No user story work can begin until this phase is complete

- [x] T004 Create `src/agent.rs` with `Agent` struct definition containing all fields from data-model.md (id, state, queues, listeners, abort_controller, modes, stream_fn, hooks, policies)
- [x] T005 [P] Implement `AgentState` struct in `src/agent.rs` with fields: system_prompt, model, tools, messages, is_running, stream_message, pending_tool_calls, error, available_models
- [x] T006 [P] Implement `Agent::new(options: AgentOptions) -> Agent` constructor that consumes `AgentOptions` and initializes all fields
- [x] T007 Implement `agent.id() -> AgentId` and `agent.state() -> &AgentState` accessors in `src/agent.rs`
- [x] T008 Implement `check_not_running()` guard in `src/agent.rs` returning `Err(AgentError::AlreadyRunning)` when `state.is_running` is true
- [x] T009 [P] Add `AgentError::AlreadyRunning`, `AgentError::NoMessages`, `AgentError::InvalidContinue`, and `AgentError::StructuredOutputFailed` variants to `src/error.rs`
- [x] T010 Update `src/lib.rs` to re-export all public types: Agent, AgentOptions, AgentState, AgentId, SubscriptionId, SteeringMode, FollowUpMode, AgentHandle

**Checkpoint**: Foundation ready - user story implementation can now begin in parallel

---

## Phase 3: User Story 1 - Send a Prompt and Get a Response (Priority: P1) MVP

**Goal**: Developer creates an agent, sends a text prompt, and gets back a result with response, stop reason, and usage via streaming, async, or sync invocation modes.

**Independent Test**: Send a prompt via each invocation mode using a mock provider and verify the result contains the expected response, stop reason, and usage.

### Tests for User Story 1

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T011 [P] [US1] Create test helpers in `tests/common/mod.rs` (MockStreamFn, MockTool, text_only_events, tool_call_events)
- [x] T012 [P] [US1] Write async prompt test in `tests/agent.rs` verifying result contains messages, stop_reason, and usage
- [x] T013 [P] [US1] Write streaming prompt test in `tests/agent.rs` verifying event stream yields lifecycle events in correct order
- [x] T014 [P] [US1] Write sync prompt test in `tests/agent.rs` verifying blocking call returns same result as async
- [x] T015 [P] [US1] Write text-with-images prompt test in `tests/agent.rs` verifying both text and image content blocks are included
- [x] T016 [P] [US1] Write concurrency guard test in `tests/agent.rs` verifying second prompt call returns `AgentError::AlreadyRunning`

### Implementation for User Story 1

- [x] T017 [US1] Implement `start_loop()` private method in `src/agent.rs` that sets `is_running`, creates CancellationToken, builds AgentContext, calls `agent_loop()`, and returns `Pin<Box<dyn Stream<Item = AgentEvent>>>`
- [x] T018 [US1] Implement `prompt_stream(input: Vec<AgentMessage>)` in `src/agent.rs` that wraps user input as messages, calls `check_not_running()`, and delegates to `start_loop()`
- [x] T019 [US1] Implement `prompt_async(input)` in `src/agent.rs` that calls `prompt_stream()` and collects the stream into `AgentResult`
- [x] T020 [US1] Implement `prompt_sync(input)` in `src/agent.rs` that creates a fresh tokio Runtime and blocks on `prompt_async()`
- [x] T021 [P] [US1] Implement convenience methods `prompt_text(text)`, `prompt_text_with_images(text, images)`, and `prompt_text_sync(text)` in `src/agent.rs`
- [x] T022 [US1] Implement `handle_stream_event(event)` in `src/agent.rs` for manual stream processing (updates state from events)

**Checkpoint**: At this point, User Story 1 should be fully functional and testable independently

---

## Phase 4: User Story 2 - Observe Agent Events (Priority: P1)

**Goal**: Developer subscribes callbacks to receive lifecycle events. Panicking subscribers are automatically removed without affecting agent or other subscribers.

**Independent Test**: Subscribe multiple callbacks, trigger a prompt, verify all receive events. A deliberately panicking subscriber should be auto-removed.

### Tests for User Story 2

- [x] T023 [P] [US2] Write subscriber receives events test in `tests/agent.rs` verifying callback gets all lifecycle events
- [x] T024 [P] [US2] Write unsubscribe test in `tests/agent.rs` verifying no further events after unsubscribe
- [x] T025 [P] [US2] Write panic isolation test in `tests/agent.rs` verifying panicking subscriber is auto-removed and agent continues
- [x] T026 [P] [US2] Write multiple subscribers test in `tests/agent.rs` verifying other subscribers unaffected by one panic

### Implementation for User Story 2

- [x] T027 [US2] Implement `subscribe(callback) -> SubscriptionId` in `src/agent_subscriptions.rs` using monotonic counter for IDs
- [x] T028 [US2] Implement `unsubscribe(id) -> bool` in `src/agent_subscriptions.rs` that removes callback by ID
- [x] T029 [US2] Implement `dispatch_event(event)` in `src/agent_subscriptions.rs` with `catch_unwind` panic isolation and auto-removal of panicking subscribers
- [x] T030 [US2] Wire `Agent::subscribe()` and `Agent::unsubscribe()` delegation methods in `src/agent.rs`
- [x] T031 [P] [US2] Implement `add_event_forwarder(f)` and `forward_event(event)` in `src/agent.rs` for event forwarding
- [x] T032 [P] [US2] Implement `emit(name, payload)` custom event emission in `src/agent.rs`

**Checkpoint**: At this point, User Stories 1 AND 2 should both work independently

---

## Phase 5: User Story 3 - Steer the Agent Mid-Run (Priority: P1)

**Goal**: Developer enqueues steering messages that redirect the agent after the current tool batch. Queue delivery modes control whether all messages or one-at-a-time are delivered.

**Independent Test**: Start a long-running tool execution, enqueue a steering message, verify the agent redirects after current tool batch.

### Tests for User Story 3

- [x] T033 [P] [US3] Write steering during tool execution test in `tests/agent_steering.rs` verifying remaining tools cancelled and message processed next turn
- [x] T034 [P] [US3] Write idle steering test in `tests/agent_steering.rs` verifying message queued for next run
- [x] T035 [P] [US3] Write all-at-once delivery mode test in `tests/agent_steering.rs` verifying multiple messages delivered together
- [x] T036 [P] [US3] Write one-at-a-time delivery mode test in `tests/agent_steering.rs` verifying single message per turn

### Implementation for User Story 3

- [x] T037 [US3] Implement `steer(message)` in `src/agent.rs` using `Arc<Mutex<Vec<AgentMessage>>>` with poison recovery
- [x] T038 [US3] Implement `follow_up(message)` in `src/agent.rs` using `Arc<Mutex<Vec<AgentMessage>>>` with poison recovery
- [x] T039 [P] [US3] Implement `clear_steering()`, `clear_follow_up()`, `clear_queues()`, and `has_pending_messages()` in `src/agent.rs`
- [x] T040 [US3] Implement `QueueMessageProvider` that shares Arc references and drains based on `SteeringMode`/`FollowUpMode`

**Checkpoint**: At this point, User Stories 1, 2, AND 3 should all work independently

---

## Phase 6: User Story 4 - Structured Output with Schema Validation (Priority: P2)

**Goal**: Developer provides a prompt and JSON schema. Agent injects a synthetic tool, validates the response, retries on invalid responses, and returns a typed result.

**Independent Test**: Mock provider returns valid structured response on first call (or invalid then valid), verify schema validation and retry behavior.

### Tests for User Story 4

- [x] T041 [P] [US4] Write valid structured output test in `tests/agent_structured.rs` verifying response matches schema
- [x] T042 [P] [US4] Write invalid-then-valid retry test in `tests/agent_structured.rs` verifying retry via continue
- [x] T043 [P] [US4] Write retries exhausted test in `tests/agent_structured.rs` verifying `StructuredOutputFailed` error with attempt count
- [x] T044 [P] [US4] Write typed deserialization test in `tests/agent_structured.rs` verifying `structured_output_typed` returns deserialized struct

### Implementation for User Story 4

- [x] T045 [US4] Implement `structured_output(prompt, schema) -> Result<Value, AgentError>` in `src/agent.rs` that injects `__structured_output` synthetic tool, runs prompt, validates response, and retries on invalid
- [x] T046 [US4] Implement `structured_output_typed<T>(prompt, schema) -> Result<T, AgentError>` in `src/agent.rs` that deserializes validated output
- [x] T047 [P] [US4] Implement `structured_output_sync(prompt, schema)` and `structured_output_typed_sync<T>(prompt, schema)` blocking variants in `src/agent.rs`

**Checkpoint**: At this point, User Stories 1-4 should all work independently

---

## Phase 7: User Story 5 - Manage Agent State (Priority: P2)

**Goal**: Developer modifies agent state between runs (system prompt, model, tools, messages) and controls agent lifecycle (abort, wait-for-idle, reset).

**Independent Test**: Modify state between prompts and verify the next prompt uses updated state. Call abort during a run and verify aborted stop reason.

### Tests for User Story 5

- [x] T048 [P] [US5] Write set system prompt test in `tests/agent.rs` verifying next prompt uses new system prompt
- [x] T049 [P] [US5] Write set model test in `tests/agent_models.rs` verifying next prompt targets new model
- [x] T050 [P] [US5] Write set tools test in `tests/agent.rs` verifying next prompt uses new tool set
- [x] T051 [P] [US5] Write abort test in `tests/handle.rs` verifying running agent exits with aborted stop reason
- [x] T052 [P] [US5] Write wait-for-idle test in `tests/handle.rs` verifying it resolves when run finishes
- [x] T053 [P] [US5] Write reset test in `tests/agent.rs` verifying all state returns to initial values

### Implementation for User Story 5

- [x] T054 [P] [US5] Implement state mutation methods in `src/agent.rs`: `set_system_prompt`, `set_model`, `set_thinking_level`, `set_tools`, `add_tool`, `remove_tool`
- [x] T055 [P] [US5] Implement message mutation methods in `src/agent.rs`: `set_messages`, `append_messages`, `clear_messages`
- [x] T056 [P] [US5] Implement `set_approval_mode(mode)` in `src/agent.rs`
- [x] T057 [US5] Implement `abort()` in `src/agent.rs` that cancels the CancellationToken
- [x] T058 [US5] Implement `wait_for_idle()` in `src/agent.rs` using `Arc<Notify>` that resolves when `is_running` becomes false
- [x] T059 [US5] Implement `reset()` in `src/agent.rs` that clears all state (messages, queues, error) to initial values

**Checkpoint**: At this point, all user stories should be independently functional

---

## Phase 8: Continue Invocations

**Purpose**: Continue operations that resume from existing context, plus validation guards

### Tests

- [x] T060 [P] [US1] Write continue async test in `tests/agent_continuation.rs` verifying resume from existing context
- [x] T061 [P] [US1] Write continue with empty history test in `tests/agent_continuation.rs` verifying `NoMessages` error
- [x] T062 [P] [US1] Write continue with assistant last message and no queue test in `tests/agent_continuation.rs` verifying `InvalidContinue` error
- [x] T063 [P] [US1] Write continue with pending queue messages test in `tests/agent_continuation.rs` verifying continue is allowed

### Implementation

- [x] T064 [US1] Implement `validate_continue()` in `src/agent.rs` returning `NoMessages` for empty history, `InvalidContinue` for assistant-last with no pending queue messages
- [x] T065 [US1] Implement `continue_stream()`, `continue_async()`, and `continue_sync()` in `src/agent.rs` using `agent_loop_continue()`

**Checkpoint**: Continue invocations work correctly with proper validation

---

## Phase 9: AgentHandle & Background Execution

**Purpose**: Spawn agent as background task with channel-based result retrieval and cancellation

### Tests

- [x] T066 [P] Write spawn and await result test in `tests/handle.rs` verifying background execution completes
- [x] T067 [P] Write cancel spawned agent test in `tests/handle.rs` verifying cancellation works

### Implementation

- [x] T068 Implement `AgentHandle` struct in `src/handle.rs` with `spawn(agent, messages)` that moves agent into `tokio::spawn`
- [x] T069 Implement `await_result()` and cancellation on `AgentHandle` in `src/handle.rs`

---

## Phase 10: Tool Discovery & Plan Mode

**Purpose**: Tool querying helpers and plan mode entry/exit

- [x] T070 [P] Implement `find_tool(name)`, `tools_matching(predicate)`, `tools_in_namespace(namespace)` in `src/agent.rs`
- [x] T071 [P] Implement `enter_plan_mode()` and `exit_plan_mode(saved_tools, saved_prompt)` in `src/agent.rs`

---

## Phase 11: Checkpointing

**Purpose**: Optional persistence via CheckpointStore trait

- [x] T072 [P] Implement `save_checkpoint(id)`, `restore_from_checkpoint(checkpoint)`, `load_and_restore_checkpoint(id)`, and `checkpoint_store()` in `src/agent.rs`

---

## Phase 12: Polish & Cross-Cutting Concerns

**Purpose**: Improvements that affect multiple user stories

- [x] T073 [P] Write public API surface test in `tests/public_api.rs` verifying all types re-exported through `src/lib.rs`
- [x] T074 Verify `src/lib.rs` re-exports are complete per contracts/public-api.md
- [x] T075 Run `cargo clippy --workspace -- -D warnings` and fix any warnings
- [x] T076 Run `cargo test --workspace` and verify all tests pass
- [x] T077 Run `cargo test -p swink-agent --no-default-features` to verify builtin-tools feature gate
- [x] T078 Run quickstart.md validation: verify all code examples compile

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies - can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion - BLOCKS all user stories
- **User Story 1 (Phase 3)**: Depends on Foundational phase completion
- **User Story 2 (Phase 4)**: Depends on Foundational phase completion - can run in parallel with US1
- **User Story 3 (Phase 5)**: Depends on Foundational phase completion - can run in parallel with US1, US2
- **User Story 4 (Phase 6)**: Depends on US1 (needs prompt/continue flow working)
- **User Story 5 (Phase 7)**: Depends on Foundational phase completion - can run in parallel with US1-3
- **Continue (Phase 8)**: Depends on US1 (needs prompt flow working)
- **AgentHandle (Phase 9)**: Depends on US1 (needs prompt flow working)
- **Tool Discovery (Phase 10)**: Depends on Foundational phase
- **Checkpointing (Phase 11)**: Depends on Foundational phase
- **Polish (Phase 12)**: Depends on all user stories being complete

### User Story Dependencies

- **User Story 1 (P1)**: No dependencies on other stories - MVP
- **User Story 2 (P1)**: No dependencies on other stories
- **User Story 3 (P1)**: No dependencies on other stories
- **User Story 4 (P2)**: Depends on US1 (uses prompt + continue flow)
- **User Story 5 (P2)**: No dependencies on other stories

### Within Each User Story

- Tests MUST be written and FAIL before implementation
- State types before methods
- Core flow before convenience wrappers
- Async before sync (sync wraps async)

### Parallel Opportunities

- T002, T003 can run in parallel (Setup phase - different files)
- T005, T006, T009 can run in parallel (Foundational phase - different concerns)
- All US1 tests (T011-T016) can run in parallel
- All US2 tests (T023-T026) can run in parallel
- All US3 tests (T033-T036) can run in parallel
- All US4 tests (T041-T044) can run in parallel
- All US5 tests (T048-T053) can run in parallel
- US1, US2, US3, and US5 can all proceed in parallel after Foundational
- US5 state mutation methods (T054-T056) can run in parallel

---

## Parallel Example: User Story 1

```bash
# Launch all tests for US1 together:
Task T011: "Create test helpers in tests/common/mod.rs"
Task T012: "Write async prompt test in tests/agent.rs"
Task T013: "Write streaming prompt test in tests/agent.rs"
Task T014: "Write sync prompt test in tests/agent.rs"
Task T015: "Write text-with-images prompt test in tests/agent.rs"
Task T016: "Write concurrency guard test in tests/agent.rs"

# Then implement sequentially:
Task T017: "Implement start_loop() - core streaming foundation"
Task T018: "Implement prompt_stream() - builds on start_loop()"
Task T019: "Implement prompt_async() - collects stream"
Task T020: "Implement prompt_sync() - blocks on async"
Task T021: "Implement convenience text methods (parallel - different signatures)"
Task T022: "Implement handle_stream_event()"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL - blocks all stories)
3. Complete Phase 3: User Story 1
4. **STOP and VALIDATE**: Test User Story 1 independently
5. Deploy/demo if ready

### Incremental Delivery

1. Complete Setup + Foundational -> Foundation ready
2. Add User Story 1 -> Test independently (MVP - basic prompt/response)
3. Add User Story 2 -> Test independently (event observation)
4. Add User Story 3 -> Test independently (steering mid-run)
5. Add User Story 4 -> Test independently (structured output)
6. Add User Story 5 -> Test independently (state management)
7. Add Continue + Handle + Tool Discovery + Checkpointing
8. Polish phase -> Full validation

### Parallel Team Strategy

With multiple developers:

1. Team completes Setup + Foundational together
2. Once Foundational is done:
   - Developer A: User Story 1 (prompt/response)
   - Developer B: User Story 2 (events/subscribers)
   - Developer C: User Story 3 (steering)
   - Developer D: User Story 5 (state management)
3. After US1 is done: User Story 4 (structured output) and Continue/Handle phases

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story should be independently completable and testable
- Verify tests fail before implementing
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
- The code is already fully implemented in `src/agent.rs`, `src/agent_options.rs`, `src/agent_subscriptions.rs`, `src/handle.rs` - tasks describe the logical implementation order
