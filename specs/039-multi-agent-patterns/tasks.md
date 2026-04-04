# Tasks: Multi-Agent Patterns Crate & Pipeline Primitives

**Input**: Design documents from `/specs/039-multi-agent-patterns/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/

**Tests**: Included — the project constitution mandates test-driven development (tests before implementation).

**Organization**: Tasks grouped by user story for independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Phase 1: Setup

**Purpose**: Crate scaffolding and workspace integration

- [x] T001 Add `swink-agent-patterns` to workspace members in root `Cargo.toml` and add any new workspace dependencies (regex is already present)
- [x] T002 Create `patterns/Cargo.toml` with `swink-agent = { path = ".." }`, workspace deps (`tokio`, `tokio-util`, `serde`, `serde_json`, `regex`, `uuid`, `tracing`, `thiserror`), and `pipelines` feature gate (default-enabled)
- [x] T003 Create `patterns/src/lib.rs` with `#![forbid(unsafe_code)]`, feature-gated `pipeline` module, and public re-exports
- [x] T004 Create `patterns/src/pipeline/mod.rs` with submodule declarations and re-exports

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core types that ALL user stories depend on — PipelineId, Pipeline enum, error types, output types, event types, and the AgentFactory trait

**CRITICAL**: No user story work can begin until this phase is complete

- [x] T005 [P] Create `PipelineId` newtype with `new()`, `generate()` (UUID), Display, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize in `patterns/src/pipeline/types.rs`
- [x] T006 [P] Create `MergeStrategy` enum (Concat { separator }, First, Fastest { n }, Custom { aggregator }) with Clone, Debug, Serialize, Deserialize in `patterns/src/pipeline/types.rs`
- [x] T007 [P] Create `ExitCondition` enum (ToolCalled { tool_name }, OutputContains { pattern, compiled }, MaxIterations) with `output_contains()` constructor that validates regex eagerly, custom Serialize/Deserialize that stores pattern string and recompiles on deser in `patterns/src/pipeline/types.rs`
- [x] T008 Create `Pipeline` enum (Sequential, Parallel, Loop variants) with all fields per data-model.md, convenience constructors (`sequential()`, `sequential_with_context()`, `parallel()`, `loop_()`, `loop_with_max()`), `with_id()` builder, `id()` and `name()` accessors, Clone, Debug, Serialize, Deserialize in `patterns/src/pipeline/types.rs`
- [x] T009 [P] Create `StepResult` struct and `PipelineOutput` struct with all fields per data-model.md in `patterns/src/pipeline/output.rs`
- [x] T010 [P] Create `PipelineError` enum with all variants (AgentNotFound, PipelineNotFound, StepFailed, MaxIterationsReached, Cancelled, InvalidExitCondition), implement Display and Error in `patterns/src/pipeline/output.rs`
- [x] T011 [P] Create `PipelineEvent` enum with all variants (Started, StepStarted, StepCompleted, Completed, Failed) and `to_emission()` method returning `swink_agent::Emission` in `patterns/src/pipeline/events.rs`
- [x] T012 Create `AgentFactory` trait (`fn create(&self, name: &str) -> Result<Agent, PipelineError>`) and `SimpleAgentFactory` struct with `new()`, `register()`, and `AgentFactory` impl in `patterns/src/pipeline/executor.rs`
- [x] T013 Create `PipelineExecutor` struct skeleton holding `Arc<dyn AgentFactory>`, `Arc<PipelineRegistry>`, and `Option<Arc<dyn Fn(PipelineEvent) + Send + Sync>>` with `new()` and `with_event_handler()` builder in `patterns/src/pipeline/executor.rs`
- [x] T014 Write unit tests for `PipelineId` (new, generate, equality, hashing, serde round-trip) in `patterns/src/pipeline/types.rs`
- [x] T015 Write unit tests for `ExitCondition` (valid regex compiles, invalid regex errors, serde round-trip recompiles regex) in `patterns/src/pipeline/types.rs`
- [x] T016 Write unit tests for `Pipeline` constructors (sequential, parallel, loop, with_id, auto-generated IDs are unique) in `patterns/src/pipeline/types.rs`
- [x] T017 Write unit tests for `SimpleAgentFactory` (register + create succeeds, unknown name returns AgentNotFound) in `patterns/src/pipeline/executor.rs`

**Checkpoint**: All foundational types compile and pass unit tests. `cargo test -p swink-agent-patterns` passes.

---

## Phase 3: User Story 4 - Pipeline Registry (Priority: P1)

**Goal**: Thread-safe in-memory registry for pipeline definitions with register, get, list, remove

**Independent Test**: Create a registry, register three pipelines, verify list/get/remove behavior

### Tests for US4

- [ ] T018 [US4] Write tests for PipelineRegistry: register returns pipeline by ID, get returns None for unknown ID, list returns all (id, name) pairs, remove deletes entry, re-register with same ID replaces silently, len/is_empty in `patterns/src/pipeline/registry.rs`

### Implementation for US4

- [x] T019 [US4] Implement `PipelineRegistry` with `RwLock<HashMap<PipelineId, Pipeline>>` — methods: `new()`, `register()` (reads ID from pipeline), `get()` (clone), `list()`, `remove()`, `len()`, `is_empty()` in `patterns/src/pipeline/registry.rs`

**Checkpoint**: Registry fully functional and tested. `cargo test -p swink-agent-patterns` passes.

---

## Phase 4: User Story 1 - Sequential Pipeline (Priority: P1) MVP

**Goal**: Execute agents in declared order, passing each agent's text output as the next agent's input. Supports `pass_context` flag.

**Independent Test**: Register two mock agents, create a sequential pipeline, execute it, verify agent-B receives agent-A's output and final result is agent-B's response.

### Tests for US1

- [ ] T020 [US1] Write test: two-step sequential pipeline passes agent-A's text output as agent-B's user message input in `patterns/src/pipeline/executor.rs`
- [ ] T021 [US1] Write test: sequential pipeline with three steps halts on step-2 error, step-3 never runs, returns StepFailed in `patterns/src/pipeline/executor.rs`
- [ ] T022 [US1] Write test: sequential pipeline with `pass_context: true` passes accumulated user/assistant text messages (no tool messages) to each step in `patterns/src/pipeline/executor.rs`
- [ ] T023 [US1] Write test: sequential pipeline referencing missing agent returns AgentNotFound in `patterns/src/pipeline/executor.rs`
- [ ] T024 [US1] Write test: zero-step sequential pipeline returns empty response and zero usage in `patterns/src/pipeline/executor.rs`

### Implementation for US1

- [ ] T025 [US1] Implement `PipelineExecutor::run()` dispatch method that matches on Pipeline variant and delegates to private `run_sequential()`, `run_parallel()`, `run_loop()` in `patterns/src/pipeline/executor.rs`
- [ ] T026 [US1] Implement `run_sequential()` — iterate steps, create fresh agent per step via factory, send user message (previous output or accumulated context), collect StepResult with timing and usage, aggregate PipelineOutput in `patterns/src/pipeline/executor.rs`
- [ ] T027 [US1] Add text extraction helper: extract concatenated text content blocks from agent result (excluding tool call metadata) in `patterns/src/pipeline/executor.rs`

**Checkpoint**: Sequential pipelines execute end-to-end. `cargo test -p swink-agent-patterns` passes.

---

## Phase 5: User Story 2 - Parallel Pipeline (Priority: P1)

**Goal**: Execute all branches concurrently, merge results via configurable strategy (Concat, First, Fastest)

**Independent Test**: Register three mock agents, create a parallel pipeline with Concat, verify all three receive the same input and outputs are joined in declaration order.

### Tests for US2

- [ ] T028 [US2] Write test: parallel pipeline with Concat merges all outputs in declaration order in `patterns/src/pipeline/executor.rs`
- [ ] T029 [US2] Write test: parallel pipeline with First returns first completed branch and cancels remaining in `patterns/src/pipeline/executor.rs`
- [ ] T030 [US2] Write test: parallel pipeline with Fastest(2) returns first two completed and cancels remaining in `patterns/src/pipeline/executor.rs`
- [ ] T031 [US2] Write test: parallel pipeline with Concat fails entirely if any branch errors (strict, no partial results) in `patterns/src/pipeline/executor.rs`
- [ ] T032 [US2] Write test: cancellation token propagates to all branches in `patterns/src/pipeline/executor.rs`
- [ ] T033 [US2] Write test: parallel pipeline with one branch works (no special-casing) in `patterns/src/pipeline/executor.rs`

### Implementation for US2

- [ ] T034 [US2] Implement `run_parallel()` — spawn each branch via `tokio::spawn` with child CancellationToken, create fresh agent per branch via factory, collect results via mpsc channel in `patterns/src/pipeline/executor.rs`
- [ ] T035 [US2] Implement Concat merge — wait for all branches, order results by declaration index, join with separator in `patterns/src/pipeline/executor.rs`
- [ ] T036 [US2] Implement First merge — return first completed result, cancel remaining branches via shared child token in `patterns/src/pipeline/executor.rs`
- [ ] T037 [US2] Implement Fastest(N) merge — collect N results, cancel remaining branches in `patterns/src/pipeline/executor.rs`

**Checkpoint**: Parallel pipelines execute with all non-Custom merge strategies. `cargo test -p swink-agent-patterns` passes.

---

## Phase 6: User Story 3 - Loop Pipeline (Priority: P1)

**Goal**: Execute body agent repeatedly until exit condition met or max_iterations reached. Accumulates context across iterations.

**Independent Test**: Register a mock agent that calls "done" tool on iteration 3, verify loop exits after exactly 3 iterations.

### Tests for US3

- [ ] T038 [US3] Write test: loop pipeline exits when body agent calls named tool (ToolCalled exit condition) in `patterns/src/pipeline/executor.rs`
- [ ] T039 [US3] Write test: loop pipeline exits when output matches regex (OutputContains exit condition) in `patterns/src/pipeline/executor.rs`
- [ ] T040 [US3] Write test: loop pipeline returns MaxIterationsReached when exit condition never met in `patterns/src/pipeline/executor.rs`
- [ ] T041 [US3] Write test: loop pipeline halts immediately on body agent error in `patterns/src/pipeline/executor.rs`
- [ ] T042 [US3] Write test: iteration 2+ receives original input plus conversation history from prior iterations in `patterns/src/pipeline/executor.rs`
- [ ] T043 [US3] Write test: loop with MaxIterations exit condition always runs to cap in `patterns/src/pipeline/executor.rs`

### Implementation for US3

- [ ] T044 [US3] Implement `run_loop()` — iterate up to max_iterations, create fresh agent per iteration via factory, build accumulated message history (original input + prior iteration user/assistant messages), check exit condition after each iteration in `patterns/src/pipeline/executor.rs`
- [ ] T045 [US3] Implement exit condition checking — ToolCalled checks agent events for tool execution with matching name, OutputContains runs compiled regex against agent text output in `patterns/src/pipeline/executor.rs`

**Checkpoint**: All three pipeline types (Sequential, Parallel, Loop) fully functional. Core MVP complete. `cargo test -p swink-agent-patterns` passes.

---

## Phase 7: User Story 5 - Pipeline as Tool (Priority: P2)

**Goal**: Wrap a pipeline as an `AgentTool` so supervisor agents can invoke pipelines as tools

**Independent Test**: Wrap a sequential pipeline as PipelineTool, add to a supervisor's tool list, verify the tool executes the pipeline and returns output as tool result text.

### Tests for US5

- [ ] T046 [US5] Write test: PipelineTool returns pipeline's final_response as tool result text in `patterns/src/pipeline/tool.rs`
- [ ] T047 [US5] Write test: PipelineTool returns error result (not panic) when pipeline fails in `patterns/src/pipeline/tool.rs`
- [ ] T048 [US5] Write test: PipelineTool schema has `input` string parameter and description derived from pipeline name in `patterns/src/pipeline/tool.rs`

### Implementation for US5

- [ ] T049 [US5] Implement `PipelineTool` struct holding `PipelineId`, `Arc<PipelineExecutor>`, and optional description with `new()` and `with_description()` constructors in `patterns/src/pipeline/tool.rs`
- [ ] T050 [US5] Implement `AgentTool` trait for `PipelineTool` — `name()` returns pipeline name, `description()` from builder or pipeline name, `schema()` with single `input` string param, `execute()` calls `executor.run()` with input from args and returns `AgentToolResult::text()` or `AgentToolResult::error()` in `patterns/src/pipeline/tool.rs`

**Checkpoint**: Pipelines can be used as tools by other agents. `cargo test -p swink-agent-patterns` passes.

---

## Phase 8: User Story 6 - Custom Aggregator Merge (Priority: P2)

**Goal**: Parallel pipeline Custom merge strategy passes all branch outputs to an aggregator agent

**Independent Test**: Register three branch agents and an aggregator, create parallel pipeline with Custom merge, verify aggregator receives labeled outputs and pipeline returns aggregator's response.

### Tests for US6

- [ ] T051 [US6] Write test: Custom merge passes all branch outputs as labeled text sections (`[agent-name]: output`) to aggregator agent in `patterns/src/pipeline/executor.rs`
- [ ] T052 [US6] Write test: Custom merge returns AgentNotFound when aggregator agent is missing in `patterns/src/pipeline/executor.rs`

### Implementation for US6

- [ ] T053 [US6] Implement Custom merge in `run_parallel()` — after all branches complete, format outputs as `[agent-name]: output` separated by blank lines, create fresh aggregator agent via factory, send formatted message, return aggregator's response in `patterns/src/pipeline/executor.rs`

**Checkpoint**: All four merge strategies functional. `cargo test -p swink-agent-patterns` passes.

---

## Phase 9: User Story 7 - Pipeline Events (Priority: P2)

**Goal**: Emit lifecycle events (Started, StepStarted, StepCompleted, Completed, Failed) through optional event callback

**Independent Test**: Register event handler on executor, run sequential pipeline, verify handler received correct events in order.

### Tests for US7

- [ ] T054 [US7] Write test: sequential pipeline emits Started, StepStarted(0), StepCompleted(0), StepStarted(1), StepCompleted(1), Completed in order in `patterns/src/pipeline/executor.rs`
- [ ] T055 [US7] Write test: failed pipeline emits Failed event with error details in `patterns/src/pipeline/executor.rs`
- [ ] T056 [US7] Write test: StepCompleted events carry agent_name, duration, and usage in `patterns/src/pipeline/executor.rs`
- [ ] T057 [US7] Write test: no events emitted when no event handler is configured (no panics, no errors) in `patterns/src/pipeline/executor.rs`
- [ ] T058 [US7] Write test: `PipelineEvent::to_emission()` produces valid `Emission` with correct name and payload in `patterns/src/pipeline/events.rs`

### Implementation for US7

- [ ] T059 [US7] Add `emit()` helper method on `PipelineExecutor` that calls event handler if present (no-op otherwise) in `patterns/src/pipeline/executor.rs`
- [ ] T060 [US7] Wire event emission into `run_sequential()`, `run_parallel()`, and `run_loop()` — emit Started at entry, StepStarted/StepCompleted around each step, Completed/Failed at exit in `patterns/src/pipeline/executor.rs`

**Checkpoint**: Full observability across all pipeline types. `cargo test -p swink-agent-patterns` passes.

---

## Phase 10: Polish & Cross-Cutting Concerns

**Purpose**: Final quality, documentation, and workspace integration

- [ ] T061 [P] Add doc comments to all public types and methods in `patterns/src/pipeline/*.rs`
- [ ] T062 [P] Add `Send + Sync` compile-time assertions for `PipelineRegistry`, `PipelineExecutor`, `PipelineTool` in `patterns/src/lib.rs`
- [ ] T063 Run `cargo clippy -p swink-agent-patterns -- -D warnings` and fix any warnings
- [ ] T064 Run `cargo test -p swink-agent-patterns` final pass — all tests green
- [ ] T065 Run `cargo build --workspace` to verify no workspace-level breakage
- [ ] T066 Verify `cargo test -p swink-agent-patterns --no-default-features` compiles (pipeline module gated)

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Phase 1 — BLOCKS all user stories
- **US4 Registry (Phase 3)**: Depends on Phase 2 — BLOCKS US1, US2, US3 (executor needs registry)
- **US1 Sequential (Phase 4)**: Depends on Phase 3
- **US2 Parallel (Phase 5)**: Depends on Phase 3 (independent of US1)
- **US3 Loop (Phase 6)**: Depends on Phase 3 (independent of US1, US2)
- **US5 Pipeline Tool (Phase 7)**: Depends on Phase 4 (needs a working executor)
- **US6 Custom Merge (Phase 8)**: Depends on Phase 5 (extends parallel pipeline)
- **US7 Events (Phase 9)**: Depends on Phase 4 (wires into executor)
- **Polish (Phase 10)**: Depends on all desired phases being complete

### User Story Dependencies

- **US4 (Registry)**: Standalone after foundational — no user story deps
- **US1 (Sequential)**: Depends on US4 only — MVP target
- **US2 (Parallel)**: Depends on US4 only — can run parallel with US1
- **US3 (Loop)**: Depends on US4 only — can run parallel with US1, US2
- **US5 (Tool)**: Depends on US1 (needs working executor) — can start after US1
- **US6 (Custom Merge)**: Depends on US2 (extends parallel) — must follow US2
- **US7 (Events)**: Depends on US1 (wires into executor) — can start after US1

### Within Each User Story

- Tests MUST be written and FAIL before implementation
- Types/structs before logic
- Core implementation before integration
- Story tests pass before moving to next

### Parallel Opportunities

- T005, T006, T007 (foundational types) can run in parallel
- T009, T010, T011 (output, error, event types) can run in parallel
- T014, T015, T016, T017 (foundational tests) can run in parallel
- US1, US2, US3 can start in parallel once US4 (registry) is complete
- T061, T062 (polish) can run in parallel

---

## Parallel Example: Foundational Types

```bash
# Launch all independent foundational types together:
Task: "T005 - Create PipelineId in patterns/src/pipeline/types.rs"
Task: "T006 - Create MergeStrategy in patterns/src/pipeline/types.rs"
Task: "T007 - Create ExitCondition in patterns/src/pipeline/types.rs"
Task: "T009 - Create PipelineOutput/StepResult in patterns/src/pipeline/output.rs"
Task: "T010 - Create PipelineError in patterns/src/pipeline/output.rs"
Task: "T011 - Create PipelineEvent in patterns/src/pipeline/events.rs"
```

## Parallel Example: Pipeline Types (after Phase 3)

```bash
# Launch all three pipeline type implementations in parallel:
Task: "US1 - Sequential pipeline executor"
Task: "US2 - Parallel pipeline executor"
Task: "US3 - Loop pipeline executor"
```

---

## Implementation Strategy

### MVP First (US4 + US1)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational types
3. Complete Phase 3: US4 (Registry)
4. Complete Phase 4: US1 (Sequential)
5. **STOP and VALIDATE**: Sequential pipeline works end-to-end
6. `cargo test -p swink-agent-patterns` all green

### Incremental Delivery

1. Setup + Foundational + Registry → Foundation ready
2. Add US1 (Sequential) → MVP — basic pipeline composition works
3. Add US2 (Parallel) → Concurrent execution
4. Add US3 (Loop) → Iterative patterns — all core pipeline types complete
5. Add US5 (Tool) → Pipeline-as-tool bridge
6. Add US6 (Custom Merge) → Advanced parallel aggregation
7. Add US7 (Events) → Full observability
8. Polish → Production ready

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Constitution requires TDD — tests written before implementation in each story phase
- Fresh agent instances are created via `AgentFactory` (not cloned from `AgentRef`)
- All tests use mock agents via `SimpleAgentFactory` with controlled behavior
- `ExitCondition::OutputContains` requires custom serde due to non-serializable `Regex`
