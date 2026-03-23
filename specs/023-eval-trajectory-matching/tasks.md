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
- [x] T011 [US1] Add acceptance test: `collect_with_guard` cancels on budget breach — verify `BudgetGuard` triggers `CancellationToken` when cost exceeds threshold, and `Invocation` trace is still complete after cancellation
- [x] T012 [US1] Add acceptance test: `BudgetGuard::from_case` returns `None` when no budget constraints defined
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

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately
- **Foundational (Phase 2)**: Depends on Setup — BLOCKS all user stories (T003 fixes FR-008 gap)
- **User Stories (Phase 3–6)**: All depend on Foundational phase completion
  - US1 and US2 are both P1, can proceed in parallel
  - US3 and US4 are both P2, can proceed in parallel
  - US4 depends on T003 (panic handling fix) for edge case test T033
- **Polish (Phase 7)**: Depends on all user stories being complete

### User Story Dependencies

- **US1 (Trajectory Collection)**: Independent — no dependencies on other stories
- **US2 (Golden Path Matching)**: Independent — no dependencies on other stories
- **US3 (Efficiency Scoring)**: Independent — no dependencies on other stories
- **US4 (Response Matching)**: T033 depends on T003 (panic fix). Otherwise independent.

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

1. Setup + Foundational → Foundation ready, FR-008 fixed
2. Add US1 → Trajectory collection verified → FR-001, FR-002, FR-009
3. Add US2 → Golden path matching verified → FR-003, FR-004
4. Add US3 → Efficiency scoring verified → FR-005
5. Add US4 → Response matching verified → FR-006, FR-007, FR-008
6. Polish → Full regression pass

---

## Notes

- [P] tasks = different files or independent test functions, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story is independently completable and testable
- The eval crate code already exists — tasks focus on closing the one code gap (FR-008 panic handling) and adding spec-mapped acceptance tests
- Commit after each phase or logical group
