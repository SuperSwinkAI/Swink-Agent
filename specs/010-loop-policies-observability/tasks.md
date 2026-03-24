# Tasks: Loop Policies & Observability

**Input**: Design documents from `/specs/010-loop-policies-observability/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/

> **Supersession Note**: Tasks for US1 (LoopPolicy), US4 (PostTurnHook), and US5 (BudgetGuard) were completed as the original design. These types are superseded by the configurable policy slot system in [031-policy-slots](../031-policy-slots/spec.md). The code delivered by these tasks remains in the codebase pending 031 implementation, at which point it will be replaced. Tasks for US2 (StreamMiddleware), US3 (MetricsCollector), and US6 (Checkpoint) remain active and unaffected.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Verify existing scaffolding and ensure all modules are wired into the crate

- [x] T001 Verify all seven source modules exist and are declared in `src/lib.rs`: `loop_policy`, `stream_middleware`, `emit`, `metrics`, `post_turn_hook`, `budget_guard`, `checkpoint`
- [x] T002 Verify all public types are re-exported from `src/lib.rs` per the public API contract in `specs/010-loop-policies-observability/contracts/public-api.md`
- [x] T003 Ensure `Cargo.toml` includes workspace dependencies needed by this feature: `serde`, `serde_json`, `tokio`, `futures`, `tracing`, `uuid` (for checkpoint IDs if used), `tokio-util`

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core types and shared infrastructure that all user stories depend on

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [x] T004 Verify `PolicyContext` struct in `src/loop_policy.rs` matches the data-model: fields `turn_index: usize`, `accumulated_usage: Usage`, `accumulated_cost: Cost`, `assistant_message: &AssistantMessage`, `stop_reason: StopReason`, lifetime parameter, `Debug` derive
- [x] T005 [P] Verify `Emission` struct in `src/emit.rs` matches the data-model: fields `name: String`, `payload: Value`, derives `Debug`, `Clone`, constructor `Emission::new(name: impl Into<String>, payload: Value)`. Add unit test `emission_new_and_clone` verifying construction, field access, `Debug` output, and `Clone` semantics
- [x] T006 [P] Verify `TurnMetrics` struct in `src/metrics.rs` matches the data-model: fields `turn_index`, `llm_call_duration`, `tool_executions`, `usage`, `cost`, `turn_duration`, derives `Debug`, `Clone`, `Serialize`, `Deserialize`
- [x] T007 [P] Verify `ToolExecMetrics` struct in `src/metrics.rs` matches the data-model: fields `tool_name`, `duration`, `success`, derives `Debug`, `Clone`, `Serialize`, `Deserialize`
- [x] T008 [P] Verify `PostTurnContext` struct in `src/post_turn_hook.rs` matches the data-model: fields `turn_index`, `assistant_message`, `tool_results`, `accumulated_usage`, `accumulated_cost`, `messages`, lifetime parameter, `Debug` derive
- [x] T009 [P] Verify `PostTurnAction` enum in `src/post_turn_hook.rs` matches the data-model: variants `Continue`, `Stop(Option<String>)`, `InjectMessages(Vec<AgentMessage>)`, `Debug` derive
- [x] T010 [P] Verify `BudgetExceeded` enum in `src/budget_guard.rs` matches the data-model: variants `Cost { limit: f64, actual: f64 }`, `Tokens { limit: u64, actual: u64 }`, derives `Debug`, `Clone`, `PartialEq`, `Display` impl
- [x] T011 [P] Verify `MapStreamFn` type alias in `src/stream_middleware.rs` matches the contract: `Arc<dyn for<'a> Fn(Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>>) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> + Send + Sync>`

**Checkpoint**: Foundation ready — all shared types verified, user story implementation can begin

---

## Phase 3: User Story 1 — Limit Agent Turns and Cost (Priority: P1) 🎯 MVP

**Goal**: Composable loop policies that prevent runaway agent loops via turn limits, cost caps, composition, and closure-based ad-hoc rules

**Independent Test**: Configure a max-turns policy of 3, run an agent that would take 5 turns, verify it stops at 3

### Implementation for User Story 1

- [x] T012 [US1] Implement `LoopPolicy` trait in `src/loop_policy.rs` with `should_continue(&self, ctx: &PolicyContext<'_>) -> bool` method, `Send + Sync` bounds
- [x] T013 [US1] Implement blanket `LoopPolicy` impl for closures `Fn(&PolicyContext<'_>) -> bool + Send + Sync` in `src/loop_policy.rs`
- [x] T014 [P] [US1] Implement `MaxTurnsPolicy` struct in `src/loop_policy.rs`: field `max_turns: usize`, `const fn new(max_turns: usize)`, `LoopPolicy` impl returning `true` when `turn_index < max_turns`, derives `Debug`, `Clone`
- [x] T015 [P] [US1] Implement `CostCapPolicy` struct in `src/loop_policy.rs`: field `max_cost: f64`, `const fn new(max_cost: f64)`, `LoopPolicy` impl returning `true` when `accumulated_cost.total <= max_cost`, derives `Debug`, `Clone`
- [x] T016 [US1] Implement `ComposedPolicy` struct in `src/loop_policy.rs`: field `policies: Vec<Box<dyn LoopPolicy>>`, `new(policies)` constructor, `LoopPolicy` impl with AND semantics (all must return `true`), manual `Debug` impl showing policy count
- [x] T017 [US1] Add unit test in `src/loop_policy.rs`: `max_turns_policy_stops_at_limit` — verifies `should_continue` returns `false` when `turn_index >= max_turns`
- [x] T018 [US1] Add unit test in `src/loop_policy.rs`: `cost_cap_policy_stops_when_exceeded` — verifies `should_continue` returns `false` when `accumulated_cost.total > max_cost`
- [x] T019 [US1] Add unit test in `src/loop_policy.rs`: `composed_policy_any_trigger_stops` — verifies composed policy stops when either constituent triggers
- [x] T020 [US1] Add unit test in `src/loop_policy.rs`: `closure_as_policy` — verifies a closure implementing `Fn(&PolicyContext) -> bool` works as a `LoopPolicy`
- [x] T021 [US1] Add integration test in `tests/loop_policy.rs`: `max_turns_terminates_loop` — configures MaxTurnsPolicy and verifies loop terminates at the correct turn
- [x] T022 [US1] Add integration test in `tests/loop_policy.rs`: `composed_policies_first_trigger_wins` — composes MaxTurns + CostCap and verifies whichever triggers first stops the loop

**Checkpoint**: Loop policies are fully functional — agents can be governed by turn limits, cost caps, composed policies, and closures

---

## Phase 4: User Story 2 — Intercept the Output Stream (Priority: P2)

**Goal**: Stream middleware using the decorator pattern to intercept, transform, or filter assistant message events

**Independent Test**: Wrap a mock stream with middleware that adds a prefix to text deltas, verify caller sees modified events

### Implementation for User Story 2

- [x] T023 [US2] Implement `StreamMiddleware` struct in `src/stream_middleware.rs`: fields `inner: Arc<dyn StreamFn>`, `map_stream: MapStreamFn`, manual `Debug` impl
- [x] T024 [US2] Implement `StreamMiddleware::new()` constructor in `src/stream_middleware.rs` accepting `Arc<dyn StreamFn>` and a stream transformation closure
- [x] T025 [P] [US2] Implement `StreamMiddleware::with_logging()` convenience constructor in `src/stream_middleware.rs` — inspects events via callback without modifying them
- [x] T026 [P] [US2] Implement `StreamMiddleware::with_map()` convenience constructor in `src/stream_middleware.rs` — transforms each event via a mapping function
- [x] T027 [P] [US2] Implement `StreamMiddleware::with_filter()` convenience constructor in `src/stream_middleware.rs` — drops events that don't match a predicate
- [x] T028 [US2] Implement `StreamFn` trait for `StreamMiddleware` in `src/stream_middleware.rs` — delegates to inner, applies `map_stream` transformation to the output stream
- [x] T029 [US2] Add integration test in `tests/stream_middleware.rs`: `middleware_transforms_events` — wraps a mock stream, verifies events are transformed
- [x] T030 [US2] Add integration test in `tests/stream_middleware.rs`: `middleware_composes` — chains two middleware layers and verifies both transformations apply in order
- [x] T031 [US2] Add integration test in `tests/stream_middleware.rs`: `filter_middleware_drops_events` — verifies filtered events are not passed to the caller

**Checkpoint**: Stream middleware is functional — events can be logged, transformed, and filtered without modifying provider adapters

---

## Phase 5: User Story 3 — Collect Metrics on Agent Performance (Priority: P2)

**Goal**: Async metrics collector trait with structured turn-level and tool-execution-level metrics

**Independent Test**: Run a multi-turn conversation, verify the metrics collector reports correct counts and latencies

### Implementation for User Story 3

- [x] T032 [US3] Implement `MetricsCollector` trait in `src/metrics.rs` with async `on_metrics(&self, metrics: &TurnMetrics) -> Pin<Box<dyn Future<Output = ()> + Send>>` method, `Send + Sync` bounds
- [x] T033 [US3] Add unit test in `src/metrics.rs`: `turn_metrics_serialization_roundtrip` — verifies `TurnMetrics` serializes and deserializes correctly via serde
- [x] T034 [US3] Add unit test in `src/metrics.rs`: `tool_exec_metrics_fields` — verifies `ToolExecMetrics` correctly captures tool name, duration, and success
- [x] T035 [US3] Add unit test in `src/metrics.rs`: `metrics_collector_receives_turn_data` — implements a mock collector and verifies it receives correct turn metrics

**Checkpoint**: Metrics collection is functional — turn-level and tool-level performance data can be observed

---

## Phase 6: User Story 4 — Execute Logic After Each Turn (Priority: P2)

**Goal**: Async post-turn hooks that execute after each turn and return actions to influence loop behavior

**Independent Test**: Register a hook that records turn data, verify it is called after each turn

### Implementation for User Story 4

- [x] T036 [US4] Implement `PostTurnHook` trait in `src/post_turn_hook.rs` with async `on_turn_end(&self, ctx: &PostTurnContext<'a>) -> Pin<Box<dyn Future<Output = PostTurnAction> + Send + 'a>>` method, `Send + Sync` bounds
- [x] T037 [US4] Add unit test in `src/post_turn_hook.rs`: `hook_receives_turn_context` — implements a mock hook and verifies it receives correct context data
- [x] T038 [US4] Add unit test in `src/post_turn_hook.rs`: `hook_continue_action` — verifies a hook returning `Continue` does not affect loop flow
- [x] T039 [US4] Add unit test in `src/post_turn_hook.rs`: `hook_stop_action` — verifies a hook returning `Stop(Some(reason))` signals loop termination
- [x] T040 [US4] Add unit test in `src/post_turn_hook.rs`: `hook_inject_messages_action` — verifies a hook returning `InjectMessages(messages)` provides messages for the next turn
- [x] T041 [US4] Add unit test in `src/post_turn_hook.rs`: `panicking_hook_is_caught` — verifies a panicking hook does not crash the system and its action is skipped

**Checkpoint**: Post-turn hooks are functional — hooks can observe turns, stop the loop, or inject messages

---

## Phase 7: User Story 5 — Guard Against Budget Overruns in Real Time (Priority: P2)

**Goal**: Pre-call budget guard that prevents LLM calls when cost or token budgets are exhausted

**Independent Test**: Set a token budget below expected response size, verify the agent is blocked when budget is exceeded

### Implementation for User Story 5

- [x] T042 [US5] Implement `BudgetGuard` struct in `src/budget_guard.rs`: fields `max_cost: Option<f64>`, `max_tokens: Option<u64>`, derives `Debug`, `Clone`, `Default`
- [x] T043 [US5] Implement `BudgetGuard::new()` as `const fn` (no limits) and `Default` impl in `src/budget_guard.rs`
- [x] T044 [P] [US5] Implement `BudgetGuard::with_max_cost(f64) -> Self` builder method as `const fn` in `src/budget_guard.rs`
- [x] T045 [P] [US5] Implement `BudgetGuard::with_max_tokens(u64) -> Self` builder method as `const fn` in `src/budget_guard.rs`
- [x] T046 [US5] Implement `BudgetGuard::check(&self, usage: &Usage, cost: &Cost) -> Result<(), BudgetExceeded>` method in `src/budget_guard.rs` — checks cost first, then tokens
- [x] T047 [US5] Add unit test in `src/budget_guard.rs`: `no_limits_always_passes` — verifies default guard passes any usage
- [x] T048 [US5] Add unit test in `src/budget_guard.rs`: `cost_limit_blocks_when_exceeded` — verifies cost budget returns `BudgetExceeded::Cost`
- [x] T049 [US5] Add unit test in `src/budget_guard.rs`: `token_limit_blocks_when_exceeded` — verifies token budget returns `BudgetExceeded::Tokens`
- [x] T050 [US5] Add unit test in `src/budget_guard.rs`: `cost_checked_before_tokens` — verifies cost is checked first when both limits are set and both exceeded
- [x] T051 [US5] Add unit test in `src/budget_guard.rs`: `budget_exceeded_display` — verifies `Display` impl for both `BudgetExceeded` variants

**Checkpoint**: Budget guard is functional — pre-call gating prevents LLM calls when budgets are exhausted

---

## Phase 8: User Story 6 — Save and Restore Loop State (Priority: P3)

**Goal**: Serializable checkpoint snapshots of agent state with async persistence trait

**Independent Test**: Run an agent for 3 turns, save a checkpoint, verify it can be restored with correct state

### Implementation for User Story 6

- [x] T052 [US6] Implement `Checkpoint` struct in `src/checkpoint.rs` per data-model: all fields, derives `Debug`, `Clone`, `Serialize`, `Deserialize`
- [x] T053 [US6] Implement `Checkpoint::new()` constructor in `src/checkpoint.rs` — accepts `id`, `system_prompt`, `provider`, `model_id`, `messages: &[AgentMessage]`, filters out `CustomMessage` variants from messages
- [x] T054 [US6] Implement `Checkpoint` builder methods in `src/checkpoint.rs`: `with_turn_count` (const fn), `with_usage`, `with_cost`, `with_metadata`
- [x] T055 [US6] Implement `Checkpoint::restore_messages()` in `src/checkpoint.rs` — converts stored `LlmMessage` back to `Vec<AgentMessage>`
- [x] T056 [P] [US6] Implement `LoopCheckpoint` struct in `src/checkpoint.rs` per data-model: all fields including `pending_messages`, `overflow_signal`, `last_assistant_message`, derives `Debug`, `Clone`, `Serialize`, `Deserialize`
- [x] T057 [US6] Implement `LoopCheckpoint::new()` constructor and builder methods in `src/checkpoint.rs`: `with_turn_index`, `with_usage`, `with_cost`, `with_pending_messages`, `with_overflow_signal`, `with_last_assistant_message`, `with_metadata`
- [x] T058 [US6] Implement `LoopCheckpoint::to_checkpoint(id)` conversion in `src/checkpoint.rs` — converts loop-level state to portable `Checkpoint`
- [x] T059 [US6] Implement `LoopCheckpoint::restore_messages()` and `restore_pending_messages()` in `src/checkpoint.rs`
- [x] T060 [US6] Implement `CheckpointStore` async trait in `src/checkpoint.rs`: `save_checkpoint`, `load_checkpoint`, `list_checkpoints`, `delete_checkpoint`, all returning `Pin<Box<dyn Future<Output = io::Result<T>> + Send>>`
- [x] T061 [US6] Add unit test in `src/checkpoint.rs`: `checkpoint_roundtrip` — creates a checkpoint, serializes to JSON, deserializes, verifies all fields match
- [x] T062 [US6] Add unit test in `src/checkpoint.rs`: `checkpoint_filters_custom_messages` — verifies `CustomMessage` variants are excluded during checkpoint creation
- [x] T063 [US6] Add unit test in `src/checkpoint.rs`: `checkpoint_restore_messages` — verifies `restore_messages()` returns correct `AgentMessage` list
- [x] T064 [US6] Add unit test in `src/checkpoint.rs`: `loop_checkpoint_to_checkpoint_conversion` — verifies `to_checkpoint()` produces a valid `Checkpoint` with correct fields
- [x] T065 [US6] Add unit test in `src/checkpoint.rs`: `loop_checkpoint_restore_pending` — verifies `restore_pending_messages()` returns correct pending messages
- [x] T066 [US6] Add unit test in `src/checkpoint.rs`: `checkpoint_metadata` — verifies `with_metadata()` correctly stores and retrieves arbitrary key-value pairs

**Checkpoint**: Checkpoints are functional — agent state can be saved, restored, and converted between formats

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Final validation, compile-time assertions, and documentation

- [x] T067 Add compile-time `Send + Sync` assertions for all public types in `src/lib.rs` or respective modules: `PolicyContext`, `MaxTurnsPolicy`, `CostCapPolicy`, `ComposedPolicy`, `StreamMiddleware`, `Emission`, `TurnMetrics`, `ToolExecMetrics`, `PostTurnContext`, `PostTurnAction`, `BudgetGuard`, `BudgetExceeded`, `Checkpoint`, `LoopCheckpoint`
- [x] T068 Run `cargo build --workspace` and verify zero compilation errors
- [x] T069 Run `cargo test --workspace` and verify all tests pass
- [x] T070 Run `cargo clippy --workspace -- -D warnings` and fix any warnings
- [x] T071 Run `cargo test -p swink-agent --no-default-features` to verify feature-gated code compiles without defaults
- [x] T072 Validate quickstart.md examples compile by spot-checking key API patterns against actual type signatures

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup — BLOCKS all user stories
- **US1 — Loop Policies (Phase 3)**: Depends on Phase 2 — no other story dependencies
- **US2 — Stream Middleware (Phase 4)**: Depends on Phase 2 — no other story dependencies
- **US3 — Metrics (Phase 5)**: Depends on Phase 2 — no other story dependencies
- **US4 — Post-Turn Hooks (Phase 6)**: Depends on Phase 2 — no other story dependencies
- **US5 — Budget Guard (Phase 7)**: Depends on Phase 2 — no other story dependencies
- **US6 — Checkpoints (Phase 8)**: Depends on Phase 2 — no other story dependencies
- **Polish (Phase 9)**: Depends on all user stories being complete

### User Story Independence

All six user stories are **fully independent** — they operate in separate source files with no cross-dependencies. After Phase 2, all stories can proceed in parallel.

### Within Each User Story

- Struct/trait definition before implementations
- Core impl before convenience constructors
- Implementation before unit tests
- Unit tests before integration tests

### Parallel Opportunities

- All Phase 2 foundational tasks (T004–T011) marked [P] can run in parallel
- All six user stories (Phases 3–8) can run in parallel after Phase 2
- Within US1: T014 and T015 can run in parallel (MaxTurns and CostCap are independent)
- Within US2: T025, T026, T027 can run in parallel (convenience constructors are independent)
- Within US5: T044, T045 can run in parallel (builder methods are independent)

---

## Parallel Example: All User Stories

```bash
# After Phase 2 completes, launch all user stories in parallel:
Story 1: T012–T022 (loop_policy.rs)
Story 2: T023–T031 (stream_middleware.rs)
Story 3: T032–T035 (metrics.rs)
Story 4: T036–T041 (post_turn_hook.rs)
Story 5: T042–T051 (budget_guard.rs)
Story 6: T052–T066 (checkpoint.rs)
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup verification
2. Complete Phase 2: Foundational type verification
3. Complete Phase 3: User Story 1 — Loop Policies
4. **STOP and VALIDATE**: `cargo test -p swink-agent loop_policy` — all policy tests pass
5. Loop governance is functional

### Incremental Delivery

1. Complete Setup + Foundational → Foundation verified
2. Add User Story 1 (Loop Policies) → Test independently → Governance MVP
3. Add User Story 5 (Budget Guard) → Test independently → Full budget enforcement
4. Add User Story 2 (Stream Middleware) → Test independently → Stream interception
5. Add User Story 3 (Metrics) + Story 4 (Post-Turn Hooks) → Test independently → Observability
6. Add User Story 6 (Checkpoints) → Test independently → Resumability

### Parallel Strategy

All six user stories touch separate files and have no cross-dependencies. With multiple agents:
- Agent A: US1 (loop_policy.rs) + US5 (budget_guard.rs) — governance pair
- Agent B: US2 (stream_middleware.rs) + US3 (metrics.rs) — observability pair
- Agent C: US4 (post_turn_hook.rs) + US6 (checkpoint.rs) — lifecycle pair

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story is independently completable and testable in its own source file
- All code already exists — tasks verify conformance to spec and add missing tests
- Commit after each phase or logical group
- Stop at any checkpoint to validate story independently
