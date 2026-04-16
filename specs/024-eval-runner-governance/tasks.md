# Tasks: Eval Runner, Scoring & Governance

**Input**: Design documents from `/specs/024-eval-runner-governance/`
**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/

**Tests**: Tests are included — the project follows TDD per constitution (Principle II).

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Crate scaffold, dependencies, error types, and shared test infrastructure

- [x] T001 Verify eval crate structure exists with `#[forbid(unsafe_code)]` and clippy lints in `eval/Cargo.toml`
- [x] T002 [P] Verify workspace dependencies (`sha2`, `regex`, `serde_yaml` optional) are declared in root `Cargo.toml` and `eval/Cargo.toml`
- [x] T003 [P] Implement `EvalError` enum with `Agent`, `CaseNotFound`, `SetNotFound`, `InvalidCase`, `Io`, `Serde`, `Yaml` variants and `From` impls in `eval/src/error.rs`
- [x] T004 [P] Create shared test helpers (`minimal_invocation`, `minimal_case` builders) in `eval/tests/common/mod.rs`
- [x] T005 Set up `eval/src/lib.rs` with module declarations and public re-exports for all types

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core data types and scoring primitives that ALL user stories depend on

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [x] T006 Implement `RecordedToolCall`, `TurnRecord`, `Invocation` recorded data types in `eval/src/types.rs`
- [x] T007 [P] Implement `ExpectedToolCall`, `ResponseCriteria` (enum with Exact/Contains/Regex/Custom variants), `BudgetConstraints` expected data types in `eval/src/types.rs`
- [x] T008 [P] Implement `Score` struct (value/threshold clamped to [0.0, 1.0]) with `pass()`, `fail()`, `new()` constructors and `verdict()` method in `eval/src/score.rs`
- [x] T009 [P] Implement `Verdict` enum (Pass/Fail) with `is_pass()` method in `eval/src/score.rs`
- [x] T010 [P] Write tests for `Score` clamping, verdict derivation, constructors in `eval/src/score.rs` (inline tests)
- [x] T011 Implement `EvalCase`, `EvalSet`, `EvalMetricResult`, `EvalCaseResult`, `EvalSetResult`, `EvalSummary` result types in `eval/src/types.rs`
- [x] T012 [P] Implement `Evaluator` trait (`name() -> &'static str`, `evaluate() -> Option<EvalMetricResult>`) in `eval/src/evaluator.rs`
- [x] T013 [P] Implement blanket `Evaluator` impl for `(&'static str, F)` closure pairs in `eval/src/evaluator.rs`
- [x] T014 Implement `TrajectoryCollector` with `observe()`, `collect_from_stream()`, `collect_with_guard()` in `eval/src/trajectory.rs`
- [x] T015 Write tests for `TrajectoryCollector` event collection and edge cases in `eval/tests/trajectory.rs`

**Checkpoint**: Foundation ready — all core types, scoring, and trajectory collection in place

---

## Phase 3: User Story 1 — Run an Evaluation Suite (Priority: P1) 🎯 MVP

**Goal**: Define eval suites in data files, run them against an agent, collect trajectories, score with evaluators, produce per-case and aggregate results.

**Independent Test**: Define a suite of three cases in a data file, run the suite, verify the report contains scores for all three cases plus aggregate results.

### Tests for User Story 1

- [x] T016 [P] [US1] Write test: runner executes multi-case suite and produces per-case scores in `eval/tests/runner.rs`
- [x] T017 [P] [US1] Write test: empty suite returns empty report (not error) in `eval/tests/runner.rs`
- [x] T018 [P] [US1] Write test: case agent failure is recorded and suite continues with remaining cases in `eval/tests/runner.rs`

### Implementation for User Story 1

- [x] T019 [US1] Implement `EvaluatorRegistry` with `new()`, `with_defaults()`, `register()`, `evaluate()` in `eval/src/evaluator.rs`
- [x] T020 [P] [US1] Implement `TrajectoryMatcher` with `Exact`, `InOrder`, `AnyOrder` match modes in `eval/src/match_.rs`
- [x] T021 [P] [US1] Implement `ResponseMatcher` with `Exact`, `Contains`, `Regex`, `Custom` criteria in `eval/src/response.rs`
- [x] T022 [US1] Implement `AgentFactory` trait in `eval/src/runner.rs`
- [x] T023 [US1] Implement `EvalRunner` with `new()`, `with_defaults()`, `run_case()` in `eval/src/runner.rs`
- [x] T024 [US1] Implement `EvalRunner::run_set()` with sequential case execution and aggregate summary in `eval/src/runner.rs`
- [x] T025 [US1] Fix `run_set()` to catch per-case agent errors, record as failed results, and continue with remaining cases (FR-003 compliance) in `eval/src/runner.rs`
- [x] T026 [P] [US1] Implement `BudgetGuard` with `from_case()`, `with_max_cost()`, `with_max_tokens()`, `with_max_turns()` for real-time budget enforcement via CancellationToken in `eval/src/trajectory.rs`
- [x] T027 [P] [US1] Write tests for `TrajectoryMatcher` all three modes with edge cases in `eval/tests/match_.rs`
- [x] T028 [P] [US1] Write tests for `ResponseMatcher` all four criteria including custom function panic handling in `eval/tests/response.rs`

**Checkpoint**: User Story 1 complete — can define suites, run them, and get scored results

---

## Phase 4: User Story 2 — Gate Deployments on Evaluation Results (Priority: P1)

**Goal**: Configure pass-rate, cost, and duration thresholds. Gate evaluates results and produces pass/fail decision reporting all violations.

**Independent Test**: Run a suite, configure a gate with specific thresholds, verify pass when met and fail when violated.

### Tests for User Story 2

- [x] T029 [P] [US2] Write test: gate passes with no config (empty thresholds) in `eval/tests/gate.rs`
- [x] T030 [P] [US2] Write test: gate fails on pass rate, cost, duration independently in `eval/tests/gate.rs`
- [x] T031 [P] [US2] Write test: multiple violations all reported in `eval/tests/gate.rs`
- [x] T032 [P] [US2] Write test: zero cases with pass rate threshold passes in `eval/tests/gate.rs`

### Implementation for User Story 2

- [x] T033 [US2] Implement `GateConfig` with `new()`, `with_min_pass_rate()`, `with_max_cost()`, `with_max_duration()` builders in `eval/src/gate.rs`
- [x] T034 [US2] Implement `GateResult` with `passed`, `exit_code`, `summary`, `exit()` in `eval/src/gate.rs`
- [x] T035 [US2] Implement `check_gate()` function that checks all thresholds and reports all violations in `eval/src/gate.rs`

**Checkpoint**: User Story 2 complete — CI/CD pipelines can gate on eval results

---

## Phase 5: User Story 3 — Register and Compose Evaluators (Priority: P2)

**Goal**: Multiple evaluators compose via registry. Custom evaluators integrate alongside built-in defaults.

**Independent Test**: Register three evaluators (two built-in, one custom), run a case, verify all three produce scores.

### Tests for User Story 3

- [x] T036 [P] [US3] Write test: registry with defaults applies all evaluators in `eval/tests/evaluator.rs`
- [x] T037 [P] [US3] Write test: custom evaluator registered alongside defaults produces score in `eval/tests/evaluator.rs`
- [x] T038 [P] [US3] Write test: evaluator returning None is excluded from results in `eval/tests/evaluator.rs`
- [x] T039 [P] [US3] Write test: case-level evaluator name filter restricts which evaluators run in `eval/tests/evaluator.rs`

### Implementation for User Story 3

- [x] T040 [US3] Verify `EvaluatorRegistry::evaluate()` filters by `case.evaluators` names when non-empty in `eval/src/evaluator.rs`
- [x] T041 [US3] Verify `EvaluatorRegistry::with_defaults()` pre-registers TrajectoryMatcher, BudgetEvaluator, ResponseMatcher, EfficiencyEvaluator in `eval/src/evaluator.rs`

**Checkpoint**: User Story 3 complete — extensible evaluator composition working

---

## Phase 6: User Story 4 — Persist and Retrieve Evaluation Results (Priority: P2)

**Goal**: Save results to filesystem, list past runs, load specific results for comparison.

**Independent Test**: Run a suite, save results, load them back, verify loaded matches originals.

### Tests for User Story 4

- [x] T042 [P] [US4] Write test: save and load eval set roundtrip in `eval/tests/store.rs`
- [x] T043 [P] [US4] Write test: save and load result roundtrip in `eval/tests/store.rs`
- [x] T044 [P] [US4] Write test: list results returns timestamps sorted ascending in `eval/tests/store.rs`
- [x] T045 [P] [US4] Write test: load non-existent set returns SetNotFound error in `eval/tests/store.rs`
- [x] T046 [P] [US4] Write test: list results on non-existent directory returns empty vec in `eval/tests/store.rs`

### Implementation for User Story 4

- [x] T047 [US4] Implement `EvalStore` trait with `save_set`, `load_set`, `save_result`, `load_result`, `list_results` in `eval/src/store.rs`
- [x] T048 [US4] Implement `FsEvalStore` with directory layout `{dir}/sets/{id}.json` and `{dir}/results/{eval_set_id}/{timestamp}.json` in `eval/src/store.rs`
- [x] T049 [US4] Implement YAML loading support (`load_set` checks `.yaml`/`.yml` before `.json`) feature-gated behind `yaml` in `eval/src/store.rs` and `eval/src/yaml.rs`
- [x] T050 [P] [US4] Write YAML loading tests in `eval/tests/yaml.rs`

**Checkpoint**: User Story 4 complete — results persist across runs, historical comparison possible

---

## Phase 7: User Story 5 — Produce Tamper-Evident Audit Trails (Priority: P3)

**Goal**: Each invocation gets SHA-256 hash chain. Verification detects tampering at the exact point of modification.

**Independent Test**: Create audit trail, verify valid chain, modify one record, verify chain breaks.

### Tests for User Story 5

- [x] T051 [P] [US5] Write test: `AuditedInvocation::from_invocation()` roundtrip verify passes in `eval/tests/audit.rs`
- [x] T052 [P] [US5] Write test: tampered turn hash fails verification in `eval/tests/audit.rs`
- [x] T053 [P] [US5] Write test: empty invocation (zero turns) creates valid audit with empty hashes in `eval/tests/audit.rs`

### Implementation for User Story 5

- [x] T054 [US5] Implement `AuditedInvocation` with `from_invocation()` computing per-turn SHA-256 hashes and chain hash in `eval/src/audit.rs`
- [x] T055 [US5] Implement `AuditedInvocation::verify()` recomputing all hashes and comparing in `eval/src/audit.rs`

**Checkpoint**: User Story 5 complete — tamper-evident audit trails with cryptographic verification

---

## Phase 8: User Story 6 — Score Resource Budget Compliance (Priority: P2)

**Goal**: Post-hoc budget evaluator scores runs on cost, tokens, turns, duration vs configured limits.

**Independent Test**: Provide runs with known values, configure budget limits, verify scores reflect over/under-budget.

### Tests for User Story 6

- [x] T056 [P] [US6] Write test: run within all budgets scores 1.0 (pass) in `eval/tests/budget.rs`
- [x] T057 [P] [US6] Write test: exceeding token budget scores 0.0 (fail) in `eval/tests/budget.rs`
- [x] T058 [P] [US6] Write test: no budget constraints returns None in `eval/tests/budget.rs`
- [x] T059 [P] [US6] Write test: exceeding multiple budgets reports all violations in details in `eval/tests/budget.rs`

### Implementation for User Story 6

- [x] T060 [US6] Implement `BudgetEvaluator` checking max_cost, max_tokens, max_turns, max_duration against invocation in `eval/src/budget.rs`
- [x] T061 [P] [US6] Implement `EfficiencyEvaluator` with duplicate ratio (0.6) + step ratio (0.4) composite scoring in `eval/src/efficiency.rs`
- [x] T062 [P] [US6] Write tests for `EfficiencyEvaluator` (no calls → None, all unique → 1.0, duplicates penalized, budget step ratio) in `eval/tests/efficiency.rs`

**Checkpoint**: User Story 6 complete — budget and efficiency scoring functional

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Final integration, documentation, and validation

- [x] T063 Update `eval/src/lib.rs` re-exports to include all public types and verify no submodule reach-through
- [x] T064 [P] Update `eval/AGENTS.md` with lessons learned and key facts from implementation
- [x] T065 [P] Run `cargo clippy --workspace -- -D warnings` and fix any warnings
- [x] T066 Run `cargo test -p swink-agent-eval` and verify all tests pass
- [x] T067 Run `cargo test -p swink-agent-eval --features yaml` and verify YAML tests pass
- [x] T068 Run quickstart.md validation — verify code examples compile and are accurate
- [x] T069 Verify `cargo test -p swink-agent-eval --no-default-features` works (no feature gates break compilation)

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **US1 (Phase 3)**: Depends on Foundational — core runner pipeline
- **US2 (Phase 4)**: Depends on Foundational — only needs `EvalSetResult` and `EvalSummary` types
- **US3 (Phase 5)**: Depends on Foundational + US1 (registry is used by runner)
- **US4 (Phase 6)**: Depends on Foundational — only needs `EvalSet` and `EvalSetResult` types
- **US5 (Phase 7)**: Depends on Foundational — only needs `Invocation` and `TurnRecord` types
- **US6 (Phase 8)**: Depends on Foundational — only needs `EvalCase`, `Invocation`, and `Score` types
- **Polish (Phase 9)**: Depends on all user stories being complete

### User Story Dependencies

- **US1 (P1)**: Can start after Foundational — no dependencies on other stories
- **US2 (P1)**: Can start after Foundational — independent of US1 (operates on result types)
- **US3 (P2)**: Depends on US1 (registry integrated into runner) — but testable independently
- **US4 (P2)**: Can start after Foundational — independent of other stories
- **US5 (P3)**: Can start after Foundational — independent of other stories
- **US6 (P2)**: Can start after Foundational — independent of other stories

### Within Each User Story

- Tests written first, verify they fail before implementation (TDD)
- Types/models before services
- Core implementation before integration
- Story complete before marking checkpoint

### Parallel Opportunities

- T002, T003, T004 can run in parallel (Phase 1)
- T006-T013 data type tasks marked [P] can run in parallel (Phase 2)
- US2, US4, US5, US6 can all start in parallel after Foundational
- All test tasks within a story marked [P] can run in parallel
- All evaluator implementations (T020, T021, T026, T060, T061) work on different files

---

## Parallel Example: User Story 1

```text
# Launch all US1 tests together (they'll fail — TDD):
T016: Write runner multi-case suite test
T017: Write empty suite test
T018: Write case failure continuation test

# After registry (T019), launch evaluator impls in parallel:
T020: TrajectoryMatcher in eval/src/match_.rs
T021: ResponseMatcher in eval/src/response.rs
T026: BudgetGuard in eval/src/trajectory.rs

# Launch evaluator tests in parallel:
T027: TrajectoryMatcher tests in eval/tests/match_.rs
T028: ResponseMatcher tests in eval/tests/response.rs
```

---

## Implementation Strategy

### MVP First (User Story 1 + User Story 2)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL — blocks all stories)
3. Complete Phase 3: User Story 1 — Run an Evaluation Suite
4. Complete Phase 4: User Story 2 — Gate Deployments
5. **STOP and VALIDATE**: Test suite execution + CI/CD gating independently
6. This delivers the core value: run evals and gate deployments

### Incremental Delivery

1. Setup + Foundational → Foundation ready
2. Add US1 (Run Suite) → Test independently → Core eval pipeline working
3. Add US2 (Gate) → Test independently → CI/CD gating enabled
4. Add US3 (Registry) + US6 (Budget) → Test independently → Extensible scoring
5. Add US4 (Persistence) → Test independently → Historical comparison
6. Add US5 (Audit) → Test independently → Governance compliance
7. Each story adds value without breaking previous stories

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Key implementation gap: T025 fixes `run_set()` to catch per-case agent errors (FR-003)
- `ResponseCriteria::Custom` is `#[serde(skip)]` — set programmatically only
- `AgentFactory` is sync because `Agent::prompt_stream()` is sync
- All evaluators return `Option` — `None` means not applicable
- Commit after each task or logical group
