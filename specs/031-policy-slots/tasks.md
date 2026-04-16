# Tasks: Configurable Policy Slots for the Agent Loop

**Input**: Design documents from `/specs/031-policy-slots/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

**TDD Note**: Per constitution principle II (Test-Driven Development), test tasks within each phase MUST be executed before their corresponding implementation tasks, regardless of task ID ordering. Write tests first, verify they fail, then implement.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Create new source files and module declarations

- [x] T001 Create `src/policy.rs` with module-level doc comment and `#[forbid(unsafe_code)]`
- [x] T002 Create `src/policies/mod.rs` with module-level doc comment and `#[forbid(unsafe_code)]`; declare submodules: `budget`, `max_turns`, `sandbox`, `deny_list`, `checkpoint`, `loop_detection`
- [x] T003 [P] Create empty stub files: `src/policies/budget.rs`, `src/policies/max_turns.rs`, `src/policies/sandbox.rs`, `src/policies/deny_list.rs`, `src/policies/checkpoint.rs`, `src/policies/loop_detection.rs`
- [x] T004 Declare `mod policy;` and `mod policies;` in `src/lib.rs`; add placeholder re-exports
- [x] T005 Create `tests/policy_slots.rs` integration test file with a `mod common;` import

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Verdict enums, context structs, slot traits, and the slot runner — the core infrastructure all user stories depend on

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

### Tests

- [x] T006 [P] Write unit tests in `src/policy.rs` `#[cfg(test)]` module: `policy_verdict_debug`, `pre_dispatch_verdict_debug`, `policy_context_construction` — verify Debug impls and struct construction
- [x] T007 [P] Write unit tests for `run_policies`: `empty_vec_returns_continue`, `single_continue`, `single_stop_short_circuits`, `inject_accumulates_across_policies`, `stop_after_inject_returns_stop`, `panic_caught_returns_continue`
- [x] T008 [P] Write unit tests for `run_pre_dispatch_policies`: `empty_vec_returns_continue`, `skip_short_circuits`, `stop_short_circuits`, `inject_accumulates`, `panic_caught_returns_continue`, `argument_mutation_visible_to_next_policy`

### Implementation

- [x] T009 Define `PolicyVerdict` enum (Continue, Stop(String), Inject(Vec<AgentMessage>)) in `src/policy.rs` with Debug, Clone derives
- [x] T010 Define `PreDispatchVerdict` enum (Continue, Stop(String), Inject(Vec<AgentMessage>), Skip(String)) in `src/policy.rs` with Debug, Clone derives
- [x] T011 [P] Define `PolicyContext<'a>` struct in `src/policy.rs`: turn_index, accumulated_usage, accumulated_cost, message_count, overflow_signal, new_messages
- [x] T012 [P] Define `ToolPolicyContext<'a>` struct in `src/policy.rs`: tool_name, tool_call_id, arguments (&mut Value)
- [x] T013 [P] Define `TurnPolicyContext<'a>` struct in `src/policy.rs`: assistant_message, tool_results, stop_reason
- [x] T014 Define four slot traits in `src/policy.rs`: `PreTurnPolicy`, `PreDispatchPolicy`, `PostTurnPolicy`, `PostLoopPolicy` — each with `name() -> &str` and `evaluate()` returning the appropriate verdict type. Bounds: `Send + Sync`
- [x] T015 Implement `run_policies()` in `src/policy.rs`: iterate vec, wrap each evaluate with `AssertUnwindSafe` + `catch_unwind`, short-circuit Stop, accumulate Inject, debug-log Stop with policy name, warn-log panics with policy name
- [x] T016 Implement `run_pre_dispatch_policies()` in `src/policy.rs`: same as T015 but with PreDispatchVerdict, additionally short-circuit Skip, pass `&mut ToolPolicyContext`
- [x] T017 Add re-exports to `src/lib.rs`: PolicyVerdict, PreDispatchVerdict, PolicyContext, ToolPolicyContext, TurnPolicyContext, all four slot traits, run_policies, run_pre_dispatch_policies
- [x] T018 Add `MockPreTurnPolicy`, `MockPreDispatchPolicy`, `MockPostTurnPolicy`, `MockPostLoopPolicy` to `tests/common/mod.rs` — configurable verdict return, call counter via AtomicUsize

**Checkpoint**: Core policy infrastructure ready — traits, verdicts, context, runner. All unit tests pass.

---

## Phase 3: User Story 1 - Budget Enforcement (Priority: P1) 🎯 MVP

**Goal**: Consumer adds BudgetPolicy to pre-turn slot; loop stops when cost/token limit exceeded

**Independent Test**: Run agent with BudgetPolicy(max_cost=1.0), verify loop stops at $1.00

### Tests

- [x] T019 [P] [US1] Write unit tests in `src/policies/budget.rs` `#[cfg(test)]`: `name_returns_budget`, `no_limits_returns_continue`, `cost_exceeded_returns_stop`, `cost_not_exceeded_returns_continue`, `token_exceeded_returns_stop`, `boundary_value_at_limit`
- [x] T020 [P] [US1] Write integration test in `tests/policy_slots.rs`: `budget_policy_stops_loop` — run a multi-turn agent with BudgetPolicy, verify loop stops and emits AgentEnd

### Implementation

- [x] T021 [US1] Implement `BudgetPolicy` struct in `src/policies/budget.rs`: `new()`, `max_cost(f64)`, `max_input_tokens(u64)`, `max_output_tokens(u64)` builder methods
- [x] T022 [US1] Implement `PreTurnPolicy` for `BudgetPolicy` in `src/policies/budget.rs`: check accumulated_cost.total vs max_cost, accumulated_usage.input_tokens vs max_input_tokens, accumulated_usage.output_tokens vs max_output_tokens
- [x] T023 [US1] Re-export `BudgetPolicy` from `src/policies/mod.rs` and `src/lib.rs`

**Checkpoint**: BudgetPolicy works standalone. Integration test proves loop stops at cost limit.

---

## Phase 4: User Story 3 - Stacking Multiple Policies (Priority: P1)

**Goal**: Multiple policies compose in a single slot — Stop short-circuits, Inject accumulates

**Independent Test**: Two policies in pre-turn slot; verify budget stops before max turns

### Tests

- [x] T024 [P] [US3] Write integration test in `tests/policy_slots.rs`: `budget_stops_before_max_turns` — BudgetPolicy + MaxTurnsPolicy in pre_turn_policies, verify budget fires first
- [x] T025 [P] [US3] Write integration test: `max_turns_stops_when_budget_ok` — same policies but budget not reached, verify max turns fires
- [x] T026 [P] [US3] Write unit test for MaxTurnsPolicy in `src/policies/max_turns.rs` `#[cfg(test)]`: `stops_at_max`, `continues_below_max`, `boundary_at_max`

### Implementation

- [x] T027 [US3] Implement `MaxTurnsPolicy` struct in `src/policies/max_turns.rs`: `new(max_turns: usize)`
- [x] T028 [US3] Implement `PreTurnPolicy` for `MaxTurnsPolicy` in `src/policies/max_turns.rs`: compare ctx.turn_index >= max_turns
- [x] T029 [US3] Implement `PostTurnPolicy` for `MaxTurnsPolicy` in `src/policies/max_turns.rs`: same check, returns PolicyVerdict
- [x] T030 [US3] Re-export `MaxTurnsPolicy` from `src/policies/mod.rs` and `src/lib.rs`

**Checkpoint**: Multiple policies in one slot compose correctly. Short-circuit and accumulation semantics verified.

---

## Phase 5: User Story 2 - Tool Access Control (Priority: P1)

**Goal**: Consumer blocks tools by name and sandboxes file paths via PreDispatch policies

**Independent Test**: Agent with ToolDenyListPolicy blocking "bash"; LLM calls bash; verify Skip error returned

### Tests

- [x] T031 [P] [US2] Write unit tests in `src/policies/deny_list.rs` `#[cfg(test)]`: `denies_listed_tool`, `allows_unlisted_tool`, `empty_deny_list_allows_all`
- [x] T032 [P] [US2] Write unit tests in `src/policies/sandbox.rs` `#[cfg(test)]`: `rejects_path_outside_root`, `allows_path_inside_root`, `handles_path_traversal_attack`, `only_checks_configured_fields`, `custom_path_fields`
- [x] T033 [P] [US2] Write integration test in `tests/policy_slots.rs`: `deny_list_skips_tool_call` — agent with ToolDenyListPolicy, verify denied tool gets Skip error result, other tools execute

### Implementation

- [x] T034 [US2] Implement `ToolDenyListPolicy` struct in `src/policies/deny_list.rs`: `new(impl IntoIterator<Item = impl Into<String>>)`
- [x] T035 [US2] Implement `PreDispatchPolicy` for `ToolDenyListPolicy`: check tool_name in denied set, return Skip with descriptive error
- [x] T036 [US2] Implement `SandboxPolicy` struct in `src/policies/sandbox.rs`: `new(impl Into<PathBuf>)`, `with_path_fields(impl IntoIterator<Item = impl Into<String>>)`. Default path_fields: `["path", "file_path", "file"]`
- [x] T037 [US2] Implement `PreDispatchPolicy` for `SandboxPolicy`: check only string values in configured `path_fields` within arguments. Skip with descriptive error if path falls outside allowed_root. Check for `..` path traversal. No silent rewriting.
- [x] T038 [US2] Re-export `ToolDenyListPolicy`, `SandboxPolicy` from `src/policies/mod.rs` and `src/lib.rs`

**Checkpoint**: PreDispatch policies block/rewrite tool calls. Approval gate never sees denied tools.

---

## Phase 6: User Story 4 - Post-Turn Persistence (Priority: P2)

**Goal**: CheckpointPolicy persists state after each turn via existing CheckpointStore

**Independent Test**: Run multi-turn agent with CheckpointPolicy, verify store.save called after each turn

### Tests

- [x] T039 [P] [US4] Write unit tests in `src/policies/checkpoint.rs` `#[cfg(test)]`: `name_returns_checkpoint`, `evaluate_returns_continue` (CheckpointPolicy always continues — persistence is a side effect)
- [x] T040 [P] [US4] Write integration test in `tests/policy_slots.rs`: `checkpoint_policy_saves_after_each_turn` — mock CheckpointStore, verify save called N times for N turns

### Implementation

- [x] T041 [US4] Implement `CheckpointPolicy` struct in `src/policies/checkpoint.rs`: `new(Arc<dyn CheckpointStore>)` (captures `Handle::current()`), `with_handle(Handle)` builder
- [x] T042 [US4] Implement `PostTurnPolicy` for `CheckpointPolicy`: build Checkpoint from TurnPolicyContext + PolicyContext, spawn save via `tokio::spawn` on captured Handle (fire-and-forget), return Continue immediately. Do not block the evaluation loop.
- [x] T043 [US4] Re-export `CheckpointPolicy` from `src/policies/mod.rs` and `src/lib.rs`

**Checkpoint**: State persisted after each turn. CheckpointStore integration verified.

---

## Phase 7: User Story 5 - Stuck Loop Detection (Priority: P2)

**Goal**: LoopDetectionPolicy detects repeated tool call patterns and injects steering or stops

**Independent Test**: Simulate 3 identical tool calls, verify policy fires after lookback window

### Tests

- [x] T044 [P] [US5] Write unit tests in `src/policies/loop_detection.rs` `#[cfg(test)]`: `no_repeat_returns_continue`, `repeat_within_lookback_returns_stop`, `repeat_with_steering_returns_inject`, `different_args_not_detected`, `lookback_window_respected`
- [x] T045 [P] [US5] Write integration test in `tests/policy_slots.rs`: `loop_detection_stops_on_repeat` — agent that repeats same tool call, verify loop stops

### Implementation

- [x] T046 [US5] Implement `LoopDetectionPolicy` struct in `src/policies/loop_detection.rs`: `new(lookback: usize)`, `with_steering(message: impl Into<String>)`, internal `Mutex<VecDeque<Vec<(String, Value)>>>` for history
- [x] T047 [US5] Implement `LoopDetectionAction` enum: `Stop`, `Inject(String)` in `src/policies/loop_detection.rs`
- [x] T048 [US5] Implement `PostTurnPolicy` for `LoopDetectionPolicy`: extract tool calls from TurnPolicyContext, compare with history, push current turn, trim to lookback window
- [x] T049 [US5] Re-export `LoopDetectionPolicy`, `LoopDetectionAction` from `src/policies/mod.rs` and `src/lib.rs`

**Checkpoint**: Loop detection works with both Stop and Inject modes. Interior mutability pattern verified.

---

## Phase 8: User Story 6 - Custom Policy Implementation (Priority: P3)

**Goal**: Verify the trait API is ergonomic for custom implementations

**Independent Test**: Implement a minimal custom PreTurnPolicy, add to agent, verify it receives correct context

### Tests

- [x] T050 [P] [US6] Write integration test in `tests/policy_slots.rs`: `custom_policy_receives_correct_context` — custom policy records PolicyContext fields, verify turn_index, usage, cost are accurate
- [x] T051 [P] [US6] Write integration test: `custom_pre_dispatch_policy_mutates_arguments` — custom policy modifies arguments, verify tool receives modified args

### Implementation

- [x] T052 [US6] No production code — this phase validates the API design. If tests reveal ergonomic issues, adjust trait signatures in `src/policy.rs`

**Checkpoint**: Custom policies work. API is ergonomic for third-party implementors.

---

## Phase 9: Loop Integration — Remove Old Fields, Wire Slots

**Purpose**: Replace old hook/guard/validator/transformer fields with policy slot vecs in the loop

### Tests

- [x] T053 Write integration test in `tests/policy_slots.rs`: `empty_policies_no_restrictions` — agent with empty vecs behaves identically to old agent with no hooks (SC-002)
- [x] T054 [P] Write integration test: `pre_dispatch_stop_aborts_batch` — PreDispatch returns Stop on tool 2 of 3, verify no tools execute
- [x] T055 [P] Write integration test: `post_turn_inject_continues_inner_loop` — PostTurn returns Inject, verify inner loop continues with injected messages
- [x] T056 [P] Write integration test: `post_loop_policy_can_stop_outer_loop` — PostLoop returns Stop, verify loop exits before follow-up poll

### Implementation

- [x] T057 Add `pre_turn_policies: Vec<Arc<dyn PreTurnPolicy>>`, `pre_dispatch_policies: Vec<Arc<dyn PreDispatchPolicy>>`, `post_turn_policies: Vec<Arc<dyn PostTurnPolicy>>`, `post_loop_policies: Vec<Arc<dyn PostLoopPolicy>>` to `AgentLoopConfig` in `src/loop_/mod.rs`
- [x] T058 Remove `budget_guard`, `loop_policy`, `post_turn_hook`, `tool_validator`, `tool_call_transformer` from `AgentLoopConfig` in `src/loop_/mod.rs`
- [x] T059 Wire PreTurn slot in `src/loop_/turn.rs` `run_single_turn()`: call `run_policies(&config.pre_turn_policies, ...)` after context transforms, before BeforeLlmCall. Handle Stop (emit TurnEnd+AgentEnd, return). Handle Inject (add to pending).
- [x] T060 Wire PostTurn slot in `src/loop_/mod.rs` `run_loop_inner()`: replace `invoke_post_turn_hook` with `run_policies(&config.post_turn_policies, ...)`. Handle Stop (break inner). Handle Inject (add to pending).
- [x] T061 Wire PostLoop slot in `src/loop_/mod.rs` `run_loop_inner()`: call `run_policies(&config.post_loop_policies, ...)` after inner loop breaks, before follow-up poll. Handle Stop (emit AgentEnd, return). Handle Inject (add to pending, continue outer).
- [x] T062 Wire PreDispatch slot in `src/loop_/tool_dispatch.rs` using two-pass approach: Pass 1 — evaluate all PreDispatch policies for all tool calls in the batch, collecting verdicts. If any verdict is Stop, abort the entire batch (no tools execute). Pass 2 — for tool calls that passed (Continue/Inject), proceed to approval gate → schema validation → execute. Skip verdicts emit error results for that tool call. Replace old approval→transformer→validator pipeline.
- [x] T063 Remove `invoke_post_turn_hook` function from `src/loop_/mod.rs`
- [x] T064 Remove old loop_policy check from inner loop in `src/loop_/mod.rs`

**Checkpoint**: Loop uses policy slots. Old fields gone. All edge cases (Stop mid-batch, Inject continues, empty vecs = no-op) verified.

---

## Phase 10: Agent & Builder Migration

**Purpose**: Update Agent struct and AgentOptions to use policy slots

- [x] T065 Replace old fields in `src/agent.rs` Agent struct: remove tool_validator, loop_policy, tool_call_transformer, post_turn_hook, budget_guard. Add pre_turn_policies, pre_dispatch_policies, post_turn_policies, post_loop_policies (all `Vec<Arc<dyn ...>>`, default empty).
- [x] T066 Replace old builder methods in `src/agent_options.rs`: remove `with_tool_validator`, `with_loop_policy`, `with_tool_call_transformer`, `with_post_turn_hook`, `with_budget_guard`, `with_cost_limit`, `with_token_limit`. Add `with_pre_turn_policy`, `with_pre_dispatch_policy`, `with_post_turn_policy`, `with_post_loop_policy`.
- [x] T067 Update `build_loop_config` in `src/agent.rs` to populate the 4 policy vecs from Agent fields
- [x] T068 Update all existing tests in `tests/` that reference old fields: rewrite to use new policy slot API. Preserve test semantics — same behavior, new API.

**Checkpoint**: Agent and builder compile with new API. All existing tests pass with updated API calls.

---

## Phase 11: Cleanup & Deletion

**Purpose**: Remove old source files and update re-exports

- [x] T069 Delete `src/budget_guard.rs`
- [x] T070 [P] Delete `src/loop_policy.rs`
- [x] T071 [P] Delete `src/post_turn_hook.rs`
- [x] T072 [P] Delete `src/tool_validator.rs`
- [x] T073 [P] Delete `src/tool_call_transformer.rs`
- [x] T074 Remove old `mod` declarations and re-exports from `src/lib.rs` for deleted modules
- [x] T075 Run `cargo clippy --workspace -- -D warnings` — fix any new warnings
- [x] T076 Run `cargo test --workspace` — verify all tests pass
- [x] T077 Run `cargo test -p swink-agent --no-default-features` — verify feature-gated compilation still works

**Checkpoint**: Zero dead code. Zero warnings. All tests green. Feature complete.

---

## Phase 12: Polish & Cross-Cutting Concerns

**Purpose**: Documentation, final validation

- [x] T078 [P] Add unit tests in `src/policy.rs` `#[cfg(test)]`: `run_policies_stop_emits_debug_trace`, `run_policies_panic_emits_warn_trace` — verify tracing output using `tracing_test` or `tracing_subscriber` test layer (FR-017, FR-018 coverage)
- [x] T079 [P] Verify negative requirements in Phase 9 code review: context transforms (FR-021), MetricsCollector (FR-023), and hardcoded mechanics (FR-022) remain unchanged after migration. Add checklist comment in T064 commit.
- [x] T080 [P] Add doc comments to all public types and trait methods in `src/policy.rs` with usage examples
- [x] T081 [P] Add doc comments to all built-in policies in `src/policies/*.rs` with builder examples
- [x] T082 Validate quickstart.md examples compile and work (spot-check 3 examples)
- [x] T083 Update `AGENTS.md` Lessons Learned section with policy slot design notes

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Phase 1 — BLOCKS all user stories
- **US1 Budget (Phase 3)**: Depends on Phase 2
- **US3 Stacking (Phase 4)**: Depends on Phase 3 (needs BudgetPolicy + MaxTurnsPolicy)
- **US2 Tool Access (Phase 5)**: Depends on Phase 2 only
- **US4 Persistence (Phase 6)**: Depends on Phase 2 only
- **US5 Loop Detection (Phase 7)**: Depends on Phase 2 only
- **US6 Custom (Phase 8)**: Depends on Phase 2 only
- **Loop Integration (Phase 9)**: Depends on Phase 2 (can start in parallel with US phases)
- **Agent Migration (Phase 10)**: Depends on Phase 9
- **Cleanup (Phase 11)**: Depends on Phase 10
- **Polish (Phase 12)**: Depends on Phase 11

### Parallel Opportunities

- **Phase 2**: T006, T007, T008 (tests) can run in parallel; T011, T012, T013 (context structs) can run in parallel
- **Phases 3-8**: US1, US2, US4, US5, US6 can all proceed in parallel after Phase 2 (US3 needs US1's BudgetPolicy)
- **Phase 9**: Can start after Phase 2 concurrently with user story phases
- **Phase 11**: T070-T073 (file deletions) can run in parallel

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (traits, verdicts, runner)
3. Complete Phase 3: User Story 1 (BudgetPolicy)
4. **STOP and VALIDATE**: BudgetPolicy stops the loop at cost limit
5. This alone delivers the most impactful policy — cost control

### Incremental Delivery

1. Setup + Foundational → Core infrastructure ready
2. Add US1 (Budget) → MVP cost control
3. Add US3 (Stacking) → Composability proven
4. Add US2 (Tool Access) → Security controls
5. Add US4+US5 (Persistence + Loop Detection) → Advanced policies
6. Add US6 (Custom) → Extensibility validated
7. Phase 9-11 (Loop Integration + Migration + Cleanup) → Full migration complete
8. Phase 12 (Polish) → Production ready

---

## Phase 13: PolicyContext Messages Extension (032 Prerequisite)

**Purpose**: Add `messages: &'a [AgentMessage]` to `PolicyContext` per updated FR-010. Backward-compatible.

### Implementation

- [x] T084 Add `pub new_messages: &'a [AgentMessage]` field to `PolicyContext<'a>` in `src/policy.rs` with doc comments explaining per-slot semantics
- [x] T085 [P] Track `new_messages_start` before pending append; pass `new_messages: &state.context_messages[new_messages_start..]` at PreTurn construction in `src/loop_/turn.rs`
- [x] T086 [P] Pass `new_messages: &[]` at PostTurn construction in `src/loop_/mod.rs` (current-turn data in TurnPolicyContext)
- [x] T087 [P] Pass `new_messages: &[]` at PostLoop construction in `src/loop_/mod.rs`
- [x] T088 [P] Pass `new_messages: &[]` at PreDispatch construction in `src/loop_/tool_dispatch.rs`
- [x] T089 Update all `make_ctx` test helpers and inline `PolicyContext` constructions to include `new_messages: &[]` in `src/policy.rs`, `src/policies/budget.rs`, `src/policies/max_turns.rs`, `src/policies/deny_list.rs`, `src/policies/sandbox.rs`, `src/policies/checkpoint.rs`, `src/policies/loop_detection.rs`
- [x] T090 Run `cargo test --workspace` — verify all tests pass with new field

### Documentation

- [x] T091 [P] Update `PolicyContext` table in `specs/031-policy-slots/data-model.md` to include `messages` field
- [x] T092 [P] Update `PolicyContext` struct in `specs/031-policy-slots/contracts/public-api.md` to include `messages` field
- [x] T093 [P] Update custom policy example in `specs/031-policy-slots/quickstart.md` to show `ctx.messages` access

**Checkpoint**: PolicyContext carries messages. All tests green. 032 can depend on this.

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Constitution requires TDD: write tests first, verify they fail, then implement
- `catch_unwind` uses `AssertUnwindSafe` wrapper — traits only need `Send + Sync`, document in trait docs (T080)
- `CheckpointPolicy` bridges sync/async via `tokio::spawn` fire-and-forget (resolved — see T042)
- PreDispatch uses two-pass approach: evaluate all tool calls first, then dispatch (T062). Stop aborts entire batch.
- SandboxPolicy checks configured field names only (default: `["path", "file_path", "file"]`), Skip with error (no silent rewrite)
