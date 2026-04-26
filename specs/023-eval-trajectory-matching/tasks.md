# Tasks: Eval Trajectory & Matching

**Input**: Design documents from `/specs/023-eval-trajectory-matching/`
**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/public-api.md

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story. The eval crate already exists with all source files — tasks focus on closing spec gaps and hardening test coverage.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Phase 1: Setup

**Purpose**: Verify existing crate compiles and passes current tests before making changes.

- [x] T001 Verify `cargo test -p swink-agent-eval` passes with all existing tests in `eval/`
- [x] T002 Verify `cargo clippy -p swink-agent-eval -- -D warnings` passes with zero warnings

**Checkpoint**: Existing code verified clean — safe to proceed with changes.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core fixes and shared test infrastructure that MUST be complete before user story acceptance tests.

- [x] T003 Fix `ResponseMatcher` panic handling: wrap `Custom` arm in `std::panic::catch_unwind` in `eval/src/response.rs`, return `Score::fail()` with panic message on catch (FR-008, research.md Gap 1)
- [x] T004 Add unit test for custom function panic in `eval/src/response.rs` `#[cfg(test)]` module: verify panicking closure returns `Score::fail()` with diagnostic details
- [x] T005 [P] Extend shared test helpers in `eval/tests/common/mod.rs`: add `case_with_trajectory_and_response()` builder, add `mock_invocation_with_response()` that sets `final_response`
- [x] T006 Verify `cargo test -p swink-agent-eval` passes after foundational changes

**Checkpoint**: Foundation ready — FR-008 gap closed, test helpers extended.

---

## Phase 3: User Story 1 — Capture Execution Traces (Priority: P1) MVP

**Goal**: TrajectoryCollector observes AgentEvent stream and builds complete Invocation traces with per-turn tool call records.

**Independent Test**: Run an agent with known tool calls, collect trajectory, verify trace contains every invocation in correct order with correct inputs/outputs.

### Implementation for User Story 1

- [x] T007 [US1] Add acceptance test: multi-tool invocation captured — create `eval/tests/trajectory.rs`, test that `TrajectoryCollector::collect_from_stream()` captures tool name, inputs for each `ToolExecutionStart` event (spec AS-1.1)
- [x] T008 [US1] Add acceptance test: multi-turn chronological ordering — verify invocations are ordered across turns via `turn_index` (spec AS-1.2)
- [x] T009 [US1] Add acceptance test: failed tool call captured — verify a `TurnEnd` with error tool results appears in the trace, not silently dropped (spec AS-1.3, FR-009)
- [x] T010 [US1] Add acceptance test: text-only response (no tool calls) — verify trace has zero `RecordedToolCall` entries but `final_response` is populated (spec AS-1.4)
- [x] T011 [US1] Add acceptance test: `collect_with_guard` cancels on budget breach — verify `BudgetGuard` triggers `CancellationToken` when cost exceeds threshold, and `Invocation` trace is still complete after cancellation **[SUPERSEDED by Phase 13 migration. Rewritten as T090 — agent-loop-level `BudgetPolicy` stops multi-turn run at cost limit, verified via `Invocation.stop_reason`.]**
- [x] T012 [US1] Add acceptance test: `BudgetGuard::from_case` returns `None` when no budget constraints defined **[SUPERSEDED by Phase 13 migration. Rewritten as T091 — `BudgetConstraints::to_policies()` returns `(None, None)` when all fields are `None`.]**
- [x] T013 [US1] Register `eval/tests/trajectory.rs` as integration test — verify all US1 tests pass with `cargo test -p swink-agent-eval --test trajectory`

**Checkpoint**: Trajectory collection fully tested against all spec acceptance scenarios.

---

## Phase 4: User Story 2 — Compare Execution Against Golden Path (Priority: P1)

**Goal**: TrajectoryMatcher compares actual trajectory against expected golden path using Exact, InOrder, or AnyOrder modes, identifying matched, missing, and unexpected steps.

**Independent Test**: Define a golden path with 3 expected steps, provide a trajectory with 2 matches plus 1 extra, verify matcher reports correctly per mode.

### Implementation for User Story 2

- [x] T014 [P] [US2] Add acceptance test: exact match — all steps matched in `eval/tests/match_.rs` (spec AS-2.1), verify `score.value == 1.0`
- [x] T015 [P] [US2] Add acceptance test: missing steps identified — trajectory missing one golden step, verify score < 1.0 and details mention missing count (spec AS-2.2)
- [x] T016 [P] [US2] Add acceptance test: extra (unexpected) steps identified — trajectory has steps not in golden path (spec AS-2.3), verify Exact mode fails, InOrder mode passes
- [x] T017 [P] [US2] Add acceptance test: ordering deviation reported — golden path in order A→B but actual is B→A (spec AS-2.4), verify Exact fails, AnyOrder passes
- [x] T018 [US2] Add edge case test: empty golden path behavior per mode — Exact returns 0.0 for non-empty actual, InOrder/AnyOrder return pass (vacuous truth) in `eval/tests/match_.rs`
- [x] T019 [US2] Add edge case test: `ExpectedToolCall` with `arguments: Some(...)` requires exact JSON equality; `arguments: None` matches by name only in `eval/tests/match_.rs`
- [x] T020 [US2] Verify all US2 tests pass with `cargo test -p swink-agent-eval --test match_`

**Checkpoint**: Golden-path comparison fully tested for all three modes and edge cases.

---

## Phase 5: User Story 3 — Score Agent Efficiency (Priority: P2)

**Goal**: EfficiencyEvaluator computes composite score from duplicate ratio (0.6 weight) and step ratio (0.4 weight).

**Independent Test**: Provide trajectories with known duplicate counts and step counts, verify scores match expected weighted calculation.

### Implementation for User Story 3

- [x] T021 [P] [US3] Add acceptance test: perfect efficiency (no duplicates, ideal turns) → score 1.0 in `eval/tests/efficiency.rs` (spec AS-3.1)
- [x] T022 [P] [US3] Add acceptance test: 50% duplicates + 2x expected turns → verify score matches weighted formula (spec AS-3.2)
- [x] T023 [P] [US3] Add acceptance test: empty trajectory (zero tool calls) → evaluator returns `None` (spec AS-3.3)
- [x] T024 [US3] Add acceptance test: compare two trajectories for same task → more efficient one scores higher (spec AS-3.4)
- [x] T025 [US3] Add edge case test: efficiency with `budget.max_turns` set — verify `ideal_turns` uses budget value, not `unique_call_count` in `eval/tests/efficiency.rs`
- [x] T025a [US3] Add determinism assertion (SC-003): in `eval/tests/efficiency.rs`, call `EfficiencyEvaluator::evaluate()` twice on the same `(case, invocation)` pair, assert `EvalMetricResult` is equal (same `score.value`, same `details`). One-line addition to an existing test is fine.
- [x] T026 [US3] Verify all US3 tests pass with `cargo test -p swink-agent-eval --test efficiency`

**Checkpoint**: Efficiency scoring verified against exact formula with all weight combinations.

---

## Phase 6: User Story 4 — Match Response Content (Priority: P2)

**Goal**: ResponseMatcher evaluates final response text against Exact, Contains, Regex, and Custom criteria.

**Independent Test**: Provide responses and criteria of each type, verify matches and mismatches correctly reported.

### Implementation for User Story 4

- [x] T027 [P] [US4] Add acceptance test: exact match pass and fail in `eval/tests/response.rs` (spec AS-4.1)
- [x] T028 [P] [US4] Add acceptance test: contains match pass and fail (spec AS-4.2)
- [x] T029 [P] [US4] Add acceptance test: regex match pass and fail (spec AS-4.3)
- [x] T030 [P] [US4] Add acceptance test: custom function match pass and fail (spec AS-4.4)
- [x] T031 [US4] Add acceptance test: custom criterion combining multiple sub-checks — verify composite pass/fail with details (spec AS-4.5)
- [x] T032 [US4] Add edge case test: invalid regex returns `Score::fail()` with compilation error in details in `eval/tests/response.rs`
- [x] T033 [US4] Add edge case test: custom function panic caught — verify `Score::fail()` returned with panic message (requires T003)
- [x] T034 [US4] Add edge case test: `None` final_response falls back to empty string matching in `eval/tests/response.rs`
- [x] T035 [US4] Verify all US4 tests pass with `cargo test -p swink-agent-eval --test response`

**Checkpoint**: Response matching verified for all four strategies and edge cases.

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: Full integration verification and cleanup.

- [x] T036 Run full `cargo test -p swink-agent-eval` — verify all unit + integration tests pass
- [x] T037 Run `cargo clippy -p swink-agent-eval -- -D warnings` — verify zero warnings after all changes
- [x] T038 Run `cargo test --workspace` — verify no regressions in other crates
- [x] T039 Verify `cargo test -p swink-agent-eval --no-default-features` compiles and runs (feature gate check)
- [x] T040 Run quickstart.md code examples mentally against public API — verify all imports and signatures are correct

---

## Phase 8: Foundational v2 — Judge & StateCapture Infrastructure (Blocking for US5–US7)

**Purpose**: Introduce the `JudgeClient` trait, `EnvironmentState` types, new `EvalCase` fields, and case-load validation shared by all three new stories. Concrete `JudgeClient` implementations (provider bindings, prompt templates, retry/backoff) are out of scope — delivered by the forthcoming "Advanced Evals" spec. 023 ships only the trait, the test double, and the evaluators that consume it.

- [x] T041 Add `eval/src/judge.rs`: define `pub trait JudgeClient: Send + Sync` with single async method `judge(&self, prompt: &str) -> Result<JudgeVerdict, JudgeError>`, plus `JudgeVerdict { score: f64, pass: bool, reason: Option<String>, label: Option<String> }` and `JudgeError { Transport, Timeout, MalformedResponse, Other }` enum variants. `#[forbid(unsafe_code)]`, no provider-specific types (FR-010).
- [x] T042 [P] Add `EnvironmentState` type to `eval/src/types.rs`: `pub struct EnvironmentState { name: String, state: serde_json::Value }` with Serialize/Deserialize + Debug + Clone.
- [x] T043 Extend `EvalCase` in `eval/src/types.rs`: add `expected_environment_state: Option<Vec<EnvironmentState>>`, `expected_tool_intent: Option<ToolIntent>` (`{ intent: String, tool_name: Option<String> }`), `semantic_tool_selection: bool` flag, and `state_capture: Option<StateCapture>` (use `#[serde(skip)]` for the callback — programmatic only, mirroring `ResponseCriteria::Custom`).
- [x] T044 [P] Add `MockJudge` test double to `eval/src/testing.rs` (no feature gate, per QA audit memory): constructor that returns canned `JudgeVerdict` sequences, plus a failing variant for error-path tests. Public API — used by US5/US6 tests.
- [x] T045 Add case-load validation in `eval/src/yaml.rs` and `EvalCase` constructor helpers: reject duplicate names in `expected_environment_state` with a clear `EvalError::InvalidCase { reason }` pointing to the offending name (FR-015, SC-009). Unit-test rejection path.
- [x] T046 Extend `EvaluatorRegistry` in `eval/src/evaluator.rs`: add `with_judge(client: Arc<dyn JudgeClient>)` and `with_defaults_and_judge(client)` constructors that register the three new evaluators in addition to existing defaults. Existing `with_defaults()` keeps current behavior (no judge, semantic evaluators return `None` — FR-011/FR-012).
- [x] T047 Wire new types into `eval/src/lib.rs` public re-exports: `JudgeClient`, `JudgeVerdict`, `JudgeError`, `EnvironmentState`, `ToolIntent`, `StateCapture` (type alias).
- [x] T048 Verify `cargo test -p swink-agent-eval` and `cargo clippy -p swink-agent-eval -- -D warnings` both pass after foundational changes.

**Checkpoint**: Judge trait, StateCapture hook, and new `EvalCase` fields in place; foundational validation and test doubles available.

---

## Phase 9: User Story 5 — Semantic Tool-Selection Scoring (Priority: P2)

**Goal**: `SemanticToolSelectionEvaluator` uses an injected `JudgeClient` to score whether each actual tool call was appropriate given the user goal, available tools, and session history. Returns `None` when judge or criterion absent.

**Independent Test**: Case where golden path expects `read_file` but agent calls `fetch_document`; deterministic matcher reports miss, semantic matcher with a `MockJudge` returning Pass accepts it.

- [x] T049 [US5] Create `eval/src/semantic_tool_selection.rs`: `SemanticToolSelectionEvaluator { judge: Arc<dyn JudgeClient>, timeout: Duration }`, implements `Evaluator`, iterates `invocation.turns[*].tool_calls`, builds prompt with goal/tools/history/chosen tool, wraps each `judge.judge(prompt)` in `tokio::time::timeout(self.timeout, ...)`, aggregates verdicts into a single `Score`. Returns `None` when `!case.semantic_tool_selection` (FR-011). Constructors: `new(judge)` sets `timeout = Duration::from_secs(300)` (5 min default); `with_timeout(Duration)` builder for override. Outer timeout elapse → `Score::fail()` with `details = "judge call exceeded {timeout:?}"` (FR-010, FR-014).
- [x] T050 [P] [US5] Acceptance test in `eval/tests/semantic_tool_selection.rs`: semantically equivalent tool accepted (AS-5.1) — `MockJudge` returns Pass, evaluator returns Pass with reason in details.
- [x] T051 [P] [US5] Acceptance test: no judge configured → `None` (AS-5.2) — use `EvaluatorRegistry::with_defaults()` (no judge) and verify semantic evaluator never appears in results.
- [x] T052 [P] [US5] Acceptance test: case with `semantic_tool_selection: false` → `None` (AS-5.3).
- [x] T053 [US5] Edge case test: malformed judge response → `Score::fail()` with parse error in details (AS-5.4). `MockJudge` returns `JudgeError::MalformedResponse`.
- [x] T054 [US5] Edge case test: transport error → `Score::fail()`, registry continues to subsequent evaluators and cases (AS-5.5). Verify via two-case eval set where case 1's judge fails and case 2 still runs.
- [x] T054a [US5] Edge case test: outer `tokio::time::timeout` elapses — use a `SlowMockJudge` that sleeps longer than the evaluator's configured timeout; construct `SemanticToolSelectionEvaluator::new(judge).with_timeout(Duration::from_millis(50))`; verify `Score::fail()` with timeout context in details, and that the test completes promptly (no hang). Covers FR-010 outer-deadline guarantee.
- [x] T055 [US5] Edge case test: empty trajectory (zero tool calls) → `None` (consistent with other evaluators — see edge case list).
- [x] T056 [US5] Verify `cargo test -p swink-agent-eval --test semantic_tool_selection` passes.

**Checkpoint**: US5 fully tested against all acceptance scenarios and edge cases; FR-011 + FR-014 coverage verified.

---

## Phase 10: User Story 6 — Semantic Tool-Parameter Scoring (Priority: P2)

**Goal**: `SemanticToolParameterEvaluator` uses the injected `JudgeClient` to score whether actual tool arguments satisfy a declared `expected_tool_intent`. Supports optional per-tool-name filter.

**Independent Test**: Case with `expected_tool_intent: "read config for project-alpha"`; agent calls with `{"path": "./project-alpha/config.toml"}`. Semantic matcher accepts; deterministic matcher would reject.

- [x] T057 [US6] Create `eval/src/semantic_tool_parameter.rs`: `SemanticToolParameterEvaluator { judge: Arc<dyn JudgeClient>, timeout: Duration }`. For each actual tool call, if `case.expected_tool_intent.tool_name` is set and doesn't match, skip that call (not Pass, not Fail). Otherwise prompt the judge with `{intent, tool_name, arguments}`, wrapping the call in `tokio::time::timeout(self.timeout, ...)`. Returns `None` when `case.expected_tool_intent.is_none()` (FR-012). Constructors mirror `SemanticToolSelectionEvaluator`: `new(judge)` defaults timeout to 5 min; `with_timeout(Duration)` override. Same outer-timeout → `Score::fail()` contract (FR-010, FR-014).
- [x] T058 [P] [US6] Acceptance test in `eval/tests/semantic_tool_parameter.rs`: intent satisfied by non-literal arguments → Pass (AS-6.1).
- [x] T059 [P] [US6] Acceptance test: no `expected_tool_intent` → `None` (AS-6.2).
- [x] T060 [US6] Edge case test: inner judge timeout → `Score::fail()` with timeout context, no hang (AS-6.3). `MockJudge` returns `JudgeError::Timeout`.
- [x] T060a [US6] Edge case test: outer `tokio::time::timeout` elapses — mirror T054a for `SemanticToolParameterEvaluator` using a `SlowMockJudge` and `with_timeout(Duration::from_millis(50))`. Verify prompt completion + `Score::fail()` details.
- [x] T061 [US6] Acceptance test: tool-name filter set, agent calls a different tool → targeted tool not present in trajectory → evaluator returns `None` (not Pass, not Fail) (AS-6.4).
- [x] T062 [US6] Acceptance test: tool-name filter set, agent calls both the target and other tools → only target is judged.
- [x] T063 [US6] Verify `cargo test -p swink-agent-eval --test semantic_tool_parameter` passes.

**Checkpoint**: US6 fully tested; FR-012 + FR-014 coverage verified.

---

## Phase 11: User Story 7 — Environment-State Assertions (Priority: P2)

**Goal**: `EnvironmentStateEvaluator` captures env state via a registered callback after the agent completes, then compares named states deterministically against `expected_environment_state`. No LLM dependency.

**Independent Test**: Case with `expected_environment_state: [{name: "created_file", state: "out.md"}]` plus a capture callback that lists the working dir. Agent writes `out.md`. Evaluator returns Pass.

- [ ] T064 [US7] Create `eval/src/environment_state.rs`: `EnvironmentStateEvaluator` (no fields). Calls `case.state_capture` (wrapped in `catch_unwind`), compares each expected named state to actual via full JSON equality. Returns `None` when callback absent or `expected_environment_state` absent (FR-013).
- [ ] T065 [P] [US7] Acceptance test in `eval/tests/environment_state.rs`: all named states match → Pass with matched names in details (AS-7.1).
- [ ] T066 [P] [US7] Acceptance test: missing expected name → Fail identifying missing name (AS-7.2).
- [ ] T067 [P] [US7] Acceptance test: value mismatch → Fail with expected and actual JSON in details (AS-7.3).
- [ ] T068 [US7] Acceptance test: case with `expected_environment_state` but no `state_capture` → `None`; eval set continues (AS-7.4).
- [ ] T069 [US7] Edge case test: capture callback panics → `Score::fail()` with panic message in details, no propagation (AS-7.5). Use `panic::catch_unwind(AssertUnwindSafe(...))` per registry convention.
- [ ] T070 [US7] Edge case test: captured snapshot contains extra names not in expected → ignored, evaluator still Pass (per edge case list).
- [ ] T071 [US7] Wire `EnvironmentStateEvaluator` into `EvaluatorRegistry::with_defaults()` — this one is deterministic so it's safe to register unconditionally (returns `None` when callback absent).
- [ ] T072 [US7] Verify `cargo test -p swink-agent-eval --test environment_state` passes.

**Checkpoint**: US7 fully tested; FR-013 + FR-014 + FR-015 + SC-007 + SC-009 coverage verified.

---

## Phase 12: Polish & Cross-Cutting Concerns (v2)

**Purpose**: Full regression pass, docs, and cross-cutting panic-isolation verification for the expanded scope.

- [x] T073 Run full `cargo test -p swink-agent-eval` — verify v1 and v2 tests all pass.
- [x] T074 Run `cargo clippy -p swink-agent-eval -- -D warnings` — verify zero warnings after all v2 changes.
- [x] T075 Run `cargo test --workspace` — verify no regressions in other crates.
- [x] T076 Update `eval/src/judge.rs` module rustdoc with a pointer to the forthcoming Advanced Evals spec as the home of concrete `JudgeClient` implementations.
- [x] T077 Update `specs/023-eval-trajectory-matching/quickstart.md` with a worked US5 + US6 + US7 example using `MockJudge` and an inline `state_capture` closure.
- [x] T078 Cross-cutting panic-isolation check: add an integration test `eval/tests/registry_panic_isolation.rs`. Construct `EvaluatorRegistry::with_defaults_and_judge(PanickingMockJudge)` — a `MockJudge` whose `judge()` panics on call. Build a single `EvalCase` with: `semantic_tool_selection = true`, `expected_tool_intent = Some(...)`, `expected_response = Some(ResponseCriteria::Custom(panicking_closure))`, `state_capture = Some(panicking_closure)`, `expected_environment_state = Some(vec![...])`. Run via `EvalRunner` against a minimal factory. Assert: (a) the run returns `Ok(EvalCaseResult)`, (b) `metric_results` contains `Score::fail()` entries for the panicking response closure, the semantic tool-selection evaluator, the semantic tool-parameter evaluator, and the env-state evaluator (four fails total), (c) no propagated panic escapes the runner, (d) `verdict == Verdict::Fail`. Covers FR-014 + SC-008.
- [x] T079 Update `specs/023-eval-trajectory-matching/data-model.md` with the new entities (JudgeClient, JudgeVerdict, JudgeError, EnvironmentState, ToolIntent, StateCapture, and the three new evaluators). **[Completed in the 2026-04-21 spec-kit analysis cycle — data-model.md now contains the v2 entity section.]**

---

## Phase 13: BudgetGuard → BudgetPolicy Migration (Full Port)

**Purpose**: Remove `BudgetGuard` entirely from `swink-agent-eval`. Delegate budget enforcement to the agent loop via `BudgetPolicy` (PreTurn slot) and `MaxTurnsPolicy` from `swink-agent-policies`, attached to the agent by the `AgentFactory` based on `EvalCase.budget`. This is a full-port migration: mid-turn cancellation and wall-clock `max_duration` are **accepted losses** — callers needing either must compose their own cancellation outside 023's surface.

**Scope boundary**: This phase runs independently of Phase 8–12 (Judge / StateCapture infrastructure). It can land before, after, or in parallel with the v2 user stories — no shared code paths.

- [ ] T080 Add `swink-agent-policies` as a dependency in `eval/Cargo.toml` (path dependency within the workspace). Remove `tokio-util` from `eval/Cargo.toml` if it becomes unused after Phase 13 completes (verify via `cargo check -p swink-agent-eval` after T086 lands).
- [ ] T081 Reshape `BudgetConstraints` in `eval/src/types.rs`: drop fields `max_tokens: Option<u64>` and `max_duration: Option<Duration>`; add `max_input: Option<u64>` and `max_output: Option<u64>`. Keep `max_cost: Option<f64>` and `max_turns: Option<usize>`. Update the struct's rustdoc to reference `BudgetPolicy` / `MaxTurnsPolicy`.
- [ ] T082 Add `BudgetConstraints::to_policies(&self) -> (Option<BudgetPolicy>, Option<MaxTurnsPolicy>)` in `eval/src/types.rs`: returns `(None, None)` when all fields are `None`; constructs `BudgetPolicy` from cost/input/output fields when any is set; constructs `MaxTurnsPolicy` from `max_turns` when set. Unit-test all four permutations inline.
- [ ] T083 [P] Update `eval/src/efficiency.rs` line 219 and any inline tests that construct `BudgetConstraints` with the removed fields. `FR-005`'s `ideal_turns` derivation from `budget.max_turns` is unchanged.
- [ ] T084 [P] Update `eval/tests/efficiency.rs` test cases to use the reshaped `BudgetConstraints` (drop `max_tokens`/`max_duration`, add `max_input`/`max_output` where relevant).
- [ ] T085 [P] Update `eval/tests/common/mod.rs::case_with_budget()` helper signature to accept the reshaped `BudgetConstraints`. Callers in tests using dropped fields are updated in sibling tasks (T084, T087).
- [ ] T086 Refactor `eval/src/runner.rs`: remove `BudgetGuard::from_case` call and `collect_with_guard` invocation. Replace with `collect_from_stream`. Remove the `BudgetGuard` import, the `CancellationToken` construction tied to the guard, and any guard-specific wiring. Document in the `AgentFactory` trait rustdoc that implementers MUST attach `case.budget.to_policies()` to the agent via `AgentOptions::with_pre_turn_policy(...)` if budget enforcement is desired.
- [ ] T087 Delete the `BudgetGuard` struct, `impl BudgetGuard`, `TrajectoryCollector::exceeds_budget`, `TrajectoryCollector::collect_with_guard`, and the `sleep_until_deadline` helper from `eval/src/trajectory.rs`. Remove all inline `#[cfg(test)]` tests that exercise `BudgetGuard` (guard-specific tests in `eval/src/trajectory.rs` lines ~355–505). Clean up now-unused imports (`CancellationToken`, `tokio_util`, `Duration` if no longer used).
- [ ] T088 Rewrite `eval/tests/budget.rs`: remove all tests that construct `BudgetGuard` directly. Keep tests that exercise `BudgetEvaluator` (evaluator-side budget scoring, not mid-stream cancellation). Add a new integration test `budget_policy_stops_multi_turn_run` that constructs an `AgentFactory` attaching `BudgetPolicy::new().max_cost(1.0)` to the agent, runs a multi-turn case, and asserts `Invocation.stop_reason` reflects the policy-stop verdict. Add `budget_constraints_to_policies_none_when_unset` verifying `BudgetConstraints::default().to_policies() == (None, None)`.
- [ ] T089 Remove `BudgetGuard` from `eval/src/lib.rs` re-exports. The `pub use trajectory::{BudgetGuard, TrajectoryCollector};` line becomes `pub use trajectory::TrajectoryCollector;`.
- [ ] T090 Replace T011's superseded test: in `eval/tests/budget.rs` (or equivalent agent-loop integration test), verify that attaching `BudgetPolicy::new().max_cost(0.01)` to a multi-turn run causes the loop to stop with a `PolicyVerdict::Stop` reason and `Invocation` is still complete with finalized turn records. This exercises the agent-loop equivalent of T011's "cancels on budget breach" contract (spec AS-1.x carries over — trajectory remains complete).
- [ ] T091 Replace T012's superseded test: in `eval/src/types.rs` inline tests, verify `BudgetConstraints::default().to_policies() == (None, None)`, and that `BudgetConstraints { max_cost: None, max_input: None, max_output: None, max_turns: None }.to_policies() == (None, None)`.
- [x] T092 Update `specs/023-eval-trajectory-matching/quickstart.md` to show the new `AgentFactory` pattern: factory reads `case.budget.to_policies()` and calls `AgentOptions::with_pre_turn_policy(...)` for each returned policy. Remove any `BudgetGuard`/`collect_with_guard` snippet from quickstart. **[Completed in the 2026-04-21 spec-kit analysis cycle.]**
- [ ] T093 Verify `cargo test -p swink-agent-eval` and `cargo clippy -p swink-agent-eval -- -D warnings` both pass after migration.
- [ ] T094 Verify `cargo test --workspace` — no regressions in other crates (particularly `runner.rs` consumers in integration tests).

**Checkpoint**: `BudgetGuard` fully removed. `BudgetConstraints` reshaped. Factory owns policy attach. Accepted-loss capabilities (`max_duration`, mid-turn cancel) documented as out-of-scope for 023.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately
- **Foundational (Phase 2)**: Depends on Setup — BLOCKS all v1 user stories (T003 fixes FR-008 gap)
- **User Stories v1 (Phase 3–6)**: All depend on Foundational phase completion
  - US1 and US2 are both P1, can proceed in parallel
  - US3 and US4 are both P2, can proceed in parallel
  - US4 depends on T003 (panic handling fix) for edge case test T033
- **Polish v1 (Phase 7)**: Depends on all v1 user stories being complete
- **Foundational v2 (Phase 8)**: Depends on Phase 7 complete — BLOCKS US5–US7 (introduces `JudgeClient`, `EnvironmentState`, new `EvalCase` fields, validation, registry constructors)
- **User Stories v2 (Phase 9–11)**: All depend on Phase 8 completion
  - US5, US6, US7 are all P2 and can proceed in parallel after Phase 8
  - US5 and US6 share the `JudgeClient` trait but exercise it independently — no inter-story task dependencies
  - US7 is fully deterministic (no judge dependency) and is the safest of the three to land first
- **Polish v2 (Phase 12)**: Depends on all v2 user stories being complete
- **Phase 13 (BudgetGuard → BudgetPolicy migration)**: Independent of Phases 8–12. Depends only on the v1-complete baseline (Phase 7). Can land at any time — recommended before Phase 12 so cross-cutting panic-isolation test (T078) runs against the final runner shape. T080–T089 are sequenced (same files); T083–T085 are `[P]` because they touch distinct files.

### User Story Dependencies

- **US1 (Trajectory Collection)**: Independent — no dependencies on other stories
- **US2 (Golden Path Matching)**: Independent — no dependencies on other stories
- **US3 (Efficiency Scoring)**: Independent — no dependencies on other stories
- **US4 (Response Matching)**: T033 depends on T003 (panic fix). Otherwise independent.
- **US5 (Semantic Tool-Selection)**: Depends on Phase 8 (T041 JudgeClient trait, T044 MockJudge, T046 registry constructor, T047 re-exports). Independent of US6/US7.
- **US6 (Semantic Tool-Parameter)**: Depends on Phase 8 (same items as US5). Independent of US5/US7.
- **US7 (Environment-State Assertions)**: Depends on Phase 8 items T042 (`EnvironmentState`), T043 (case fields incl. `state_capture`), T045 (validation), T047 (re-exports). Does NOT need T041/T044 (no judge). Can land before US5/US6 if desired.

### Within Each User Story

- Acceptance tests first (TDD per constitution)
- Edge case tests after acceptance tests
- Verification checkpoint last

### Parallel Opportunities

- T014, T015, T016, T017 (US2 acceptance tests — different scenarios, same file but independent tests)
- T021, T022, T023 (US3 acceptance tests — different scenarios)
- T027, T028, T029, T030 (US4 acceptance tests — different strategies)
- US1 and US2 can proceed in parallel after Phase 2
- US3 and US4 can proceed in parallel after Phase 2
- T042 (`EnvironmentState` type) and T044 (`MockJudge`) within Phase 8 — different files, no ordering constraint
- T050, T051, T052 within US5 — independent acceptance tests in the same file
- T058, T059 within US6 — independent acceptance tests
- T065, T066, T067 within US7 — independent acceptance tests
- US5, US6, US7 can all proceed in parallel after Phase 8 — three different evaluator modules and three different test files

---

## Parallel Example: User Story 2

```bash
# Launch all acceptance tests for US2 together:
Task: "T014 [P] [US2] Add acceptance test: exact match"
Task: "T015 [P] [US2] Add acceptance test: missing steps"
Task: "T016 [P] [US2] Add acceptance test: extra steps"
Task: "T017 [P] [US2] Add acceptance test: ordering deviation"
```

---

## Implementation Strategy

### MVP First (User Story 1 + Foundational Fix)

1. Complete Phase 1: Verify existing code (T001–T002)
2. Complete Phase 2: Fix panic handling + extend helpers (T003–T006)
3. Complete Phase 3: US1 Trajectory Collection tests (T007–T013)
4. **STOP and VALIDATE**: `cargo test -p swink-agent-eval` all green
5. Spec compliance: FR-001, FR-002, FR-008, FR-009 verified

### Incremental Delivery

1. Setup + Foundational v1 → Foundation ready, FR-008 fixed
2. Add US1 → Trajectory collection verified → FR-001, FR-002, FR-009
3. Add US2 → Golden path matching verified → FR-003, FR-004
4. Add US3 → Efficiency scoring verified → FR-005
5. Add US4 → Response matching verified → FR-006, FR-007, FR-008
6. Polish v1 → Full regression pass (v1 done)
7. **Foundational v2** → JudgeClient trait, EnvironmentState types, validation, registry constructors → FR-010, FR-015 prerequisites
8. **Add US7 first** (deterministic, lowest risk) → Environment-state assertions verified → FR-013, FR-015, SC-007, SC-009
9. Add US5 → Semantic tool-selection verified → FR-011
10. Add US6 → Semantic tool-parameter verified → FR-012
11. Polish v2 → Cross-cutting panic-isolation test, quickstart update, regression pass → FR-014, SC-006, SC-008

---

## Notes

- [P] tasks = different files or independent test functions, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story is independently completable and testable
- The eval crate code already exists — v1 tasks focused on closing the one code gap (FR-008 panic handling) and adding spec-mapped acceptance tests
- v2 (Phases 8–12, T041–T079) introduces NEW code: `JudgeClient` trait, `EnvironmentState`/`ToolIntent`/`StateCapture` types, three new evaluator modules, case-load validation, and evaluator-side `tokio::time::timeout` wrapping (default 5 min, configurable via `with_timeout`)
- The `JudgeClient` trait shape is intentionally minimal — concrete implementations (model providers, prompt template registries, retry/backoff, batching, caching) are out of scope and live in spec 043 (`043-evals-adv-features`). 023 ships only the trait + a `MockJudge` test double + the outer evaluator-side timeout guard
- Phase 13 (T080–T094) ports budget enforcement from `BudgetGuard` to `BudgetPolicy`/`MaxTurnsPolicy` from `swink-agent-policies`. Wall-clock deadline and mid-turn cancellation capabilities are dropped (accepted losses). Runner no longer enforces budget — responsibility shifts to `AgentFactory` implementers via `BudgetConstraints::to_policies()`
- Commit after each phase or logical group
