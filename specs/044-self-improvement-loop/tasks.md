# Tasks: Self-Improvement Loop

**Input**: Design documents from `/specs/044-self-improvement-loop/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/public-api.md, quickstart.md

**Tests**: Tests are included per user story. TDD approach — tests written before implementation.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

**Important notes**:
- The evolve crate is a new workspace member. Phase 1 must complete before any user story work begins.
- All types use `swink-agent` and `swink-agent-eval` public re-exports only — no internal imports.
- `MutationStrategy::mutate()` is synchronous. `LlmGuided` bridges to async `JudgeClient::judge()` via `tokio::runtime::Handle::current().block_on()`. Callers must invoke from an async context with a multi-threaded runtime.
- `CycleBudget` uses interior mutability (`Mutex<Cost>`) and is passed as `Arc<CycleBudget>`.
- Spec acceptance scenarios US1-AS4 and US3-AS4 (budget exhaustion) are integration-level concerns tested in US7's end-to-end tests.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Phase 1: Setup (Crate Scaffold & Dependencies)

**Purpose**: Create the workspace crate and configure dependencies

- [ ] T001 Add `swink-agent-evolve` to workspace members in root `Cargo.toml` (`[workspace]` members list, add `"evolve"`)
- [ ] T002 Create `evolve/Cargo.toml` with dependencies: `swink-agent` (path `..`, version `0.9.0`), `swink-agent-eval` (path `../eval`, version `0.9.0`, features `["judge-core"]`), `sha2` (workspace), `serde` (workspace), `serde_json` (workspace), `regex` (workspace), `tracing` (workspace), `chrono` (workspace, features `["serde"]`), `thiserror` (workspace), `tokio` (workspace). Optional: `opentelemetry` (workspace, optional). Features: `otel = ["dep:opentelemetry"]`, `all = ["otel"]`. Dev-dependencies: `tokio` (full), `tempfile` (workspace), `pretty_assertions` (workspace)
- [ ] T003 Create `evolve/src/lib.rs` with `#![forbid(unsafe_code)]`, module declarations for `config`, `diagnose`, `mutate`, `strategies`, `evaluate`, `gate`, `persist`, `runner`, `types`, and placeholder re-exports per `contracts/public-api.md`

---

## Phase 2: Foundation (Core Types & Traits)

**Purpose**: Define all shared types used across user stories. These must compile before any user story work.

- [ ] T004 [P] Implement `PromptSection` struct in `evolve/src/config.rs`: fields `name: Option<String>`, `content: String`, `byte_range: Range<usize>`. Derive `Debug, Clone, PartialEq`
- [ ] T005 [P] Implement `OptimizationTarget` in `evolve/src/config.rs`: fields `system_prompt`, `sections`, `tool_schemas`, `section_delimiter`. Constructor `new()` that auto-parses markdown `## ` headers into sections. Methods: `with_section_delimiter()`, `sections()`, `system_prompt()`, `tool_schemas()`, `with_replaced_section()`, `with_replaced_tool()`. Unit tests for section parsing: structured prompt (3 sections), unstructured prompt (1 unnamed section), custom delimiter
- [ ] T006 [P] Implement `CycleBudget` in `evolve/src/config.rs`: fields `max_cost: Cost`, `spent: Mutex<Cost>`. Methods: `new()`, `record()`, `remaining()`, `is_exhausted()`. Unit tests for budget tracking and exhaustion detection
- [ ] T007 [P] Implement `OptimizationConfig` in `evolve/src/config.rs`: builder pattern with `new(eval_set, output_root)` + `with_strategies()`, `with_acceptance_threshold()` (default 0.01), `with_budget()`, `with_parallelism()` (default 1), `with_seed()`, `with_max_weak_points()` (default 5), `with_max_candidates_per_strategy()` (default 3)
- [ ] T008 [P] Implement `TargetComponent` enum in `evolve/src/diagnose.rs`: variants `PromptSection { index, name }`, `ToolDescription { tool_name }`, `FullPrompt`. Derive `Debug, Clone, PartialEq, Serialize, Deserialize`
- [ ] T009 [P] Implement `CaseFailure` struct in `evolve/src/diagnose.rs`: fields `case_id`, `evaluator_name`, `score`, `details`. Derive `Debug, Clone`
- [ ] T010 [P] Implement `WeakPoint` struct in `evolve/src/diagnose.rs`: fields `component`, `affected_cases`, `mean_score_gap`, `severity`. Derive `Debug, Clone`
- [ ] T011 [P] Implement `MutationStrategy` trait, `MutationContext` struct, `MutationError` enum, and `Candidate` struct in `evolve/src/mutate.rs` per data-model.md. `Candidate.id` is SHA-256 hash of `mutated_value`. Unit test for candidate ID determinism
- [ ] T012 [P] Implement `AcceptanceVerdict` enum in `evolve/src/gate.rs`: variants `Accepted`, `AcceptedNotApplied`, `BelowThreshold { improvement, threshold }`, `P1Regression { case_id }`, `NoImprovement`. Derive `Debug, Clone, Serialize, Deserialize`
- [ ] T013 [P] Implement `AcceptanceResult` struct in `evolve/src/gate.rs`: fields `applied`, `accepted_not_applied`, `rejected` per data-model.md
- [ ] T014 [P] Implement `BaselineSnapshot` struct in `evolve/src/types.rs`: fields `target`, `results`, `aggregate_score`, `cost`. Method `aggregate_from_results()` computing arithmetic mean
- [ ] T015 [P] Implement `CycleStatus` enum and `CycleResult` struct in `evolve/src/types.rs` per data-model.md
- [ ] T016 [P] Implement `ManifestEntry` struct in `evolve/src/persist.rs`: all fields per data-model.md. Derive `Serialize, Deserialize`. Unit test for JSONL round-trip
- [ ] T017 [P] Implement `CandidateResult` struct in `evolve/src/evaluate.rs`: fields `candidate`, `results`, `aggregate_score`, `cost`
- [ ] T018 Update `evolve/src/lib.rs` re-exports to include all types from T004–T017

---

## Phase 3: US1 — Baseline Evaluation (Priority: P1)

**Story goal**: Run the eval suite against the original configuration and produce a scored snapshot.

**Independent test**: Construct an `OptimizationTarget`, run `baseline()` with a mock agent factory and small eval set, verify `BaselineSnapshot` scores match direct eval results.

- [ ] T019 [US1] Write test `evolve/tests/baseline.rs`: test `baseline_returns_per_case_scores` — create a mock `AgentFactory` returning a fixed agent, define 3 eval cases, run `baseline()`, assert `BaselineSnapshot` has 3 results with expected scores
- [ ] T020 [US1] Write test `evolve/tests/baseline.rs`: test `baseline_aggregate_is_arithmetic_mean` — 3 cases with scores 0.8, 0.6, 1.0, verify aggregate = 0.8
- [ ] T021 [US1] Write test `evolve/tests/baseline.rs`: test `baseline_records_failures_with_details` — include a failing case, verify failure details propagated
- [ ] T022 [US1] Implement `EvolutionRunner::new()` in `evolve/src/runner.rs`: accepts `OptimizationTarget`, `OptimizationConfig`, `Arc<dyn AgentFactory>`, `Option<Arc<dyn JudgeClient>>`. Stores fields and initializes `EvalRunner` from config
- [ ] T023 [US1] Implement `EvolutionRunner::baseline()` in `evolve/src/runner.rs`: run `EvalRunner::run_set()` with the target's system prompt and tools, collect `EvalSetResult`, compute aggregate score, return `BaselineSnapshot`. Add `#[cfg_attr(feature = "otel", tracing::instrument)]`
- [ ] T024 [US1] Implement `MutatingAgentFactory` in `evolve/src/evaluate.rs`: wraps `Arc<dyn AgentFactory>`, holds replacement system prompt and/or tool schemas. `create_agent()` delegates to inner factory after swapping the configured fields
- [ ] T025 [US1] Verify all 3 baseline tests pass

---

## Phase 4: US2 — Diagnose Weak Points (Priority: P1)

**Story goal**: Analyze baseline results and produce a ranked list of improvement opportunities.

**Independent test**: Construct a `BaselineSnapshot` with known failures, verify `Diagnoser` ranks weak points correctly.

- [ ] T026 [US2] Write test `evolve/tests/diagnose.rs`: test `diagnose_identifies_tool_failure` — 3 cases failing on same tool, verify single `WeakPoint` with `TargetComponent::ToolDescription`
- [ ] T027 [US2] Write test `evolve/tests/diagnose.rs`: test `diagnose_identifies_prompt_failure` — 1 case failing on response quality, verify `WeakPoint` with `TargetComponent::PromptSection` or `FullPrompt`
- [ ] T028 [US2] Write test `evolve/tests/diagnose.rs`: test `diagnose_returns_empty_for_passing_baseline` — all cases score > 0.9, verify empty weak point list
- [ ] T029 [US2] Write test `evolve/tests/diagnose.rs`: test `diagnose_ranks_by_severity` — two weak points with different affected_cases * mean_score_gap, verify ordering
- [ ] T030 [US2] Implement `Diagnoser::diagnose()` in `evolve/src/diagnose.rs`: accepts `&BaselineSnapshot` and `&OptimizationTarget`. Groups failing cases by evaluator type and target component. Produces `Vec<WeakPoint>` ranked by `affected_case_count * mean_score_gap`. Respects `max_weak_points` cap. Add `#[cfg_attr(feature = "otel", tracing::instrument)]`
- [ ] T031 [US2] Verify all 4 diagnosis tests pass

---

## Phase 5: US3 — Generate Candidate Mutations (Priority: P1)

**Story goal**: Produce candidate configurations from weak points using three strategies.

**Independent test**: Provide a weak point, verify each strategy produces valid candidates.

- [ ] T032 [US3] Write test `evolve/tests/mutate.rs`: test `template_based_produces_candidates` — provide a system prompt section, verify `TemplateBased::mutate()` returns at least 1 candidate with modified text
- [ ] T033 [US3] Write test `evolve/tests/mutate.rs`: test `ablation_produces_two_candidates` — provide a multi-sentence section, verify `Ablation::mutate()` returns one removed and one simplified candidate
- [ ] T034 [US3] Write test `evolve/tests/mutate.rs`: test `llm_guided_uses_judge_client` — mock `JudgeClient` returning fixed rewrite, verify `LlmGuided::mutate()` returns candidate with the judge's rewrite
- [ ] T035 [US3] Write test `evolve/tests/mutate.rs`: test `deterministic_seed_produces_identical_results` — run `TemplateBased` and `Ablation` twice with same seed, verify identical candidates
- [ ] T036 [US3] Write test `evolve/tests/mutate.rs`: test `candidates_deduplicated_by_hash` — two strategies produce same text, verify dedup reduces to 1 candidate
- [ ] T037 [US3] Write test `evolve/tests/mutate.rs`: test `max_candidates_per_strategy_enforced` — set cap to 1, verify each strategy returns at most 1 candidate
- [ ] T038 [US3] Implement `TemplateBased` strategy in `evolve/src/strategies/template_based.rs`: built-in library of ~10 find-replace templates (imperative↔declarative, verbose↔concise, etc.). `mutate()` applies templates to target, respects seed for subset selection and `max_candidates` cap. Builder method `with_template(find, replace) -> Result<Self, regex::Error>` for user-provided templates per FR-010
- [ ] T039 [US3] Implement `Ablation` strategy in `evolve/src/strategies/ablation.rs`: produces 2 candidates per target — full removal and first-sentence simplification. Respects `max_candidates` cap
- [ ] T040 [US3] Implement `LlmGuided` strategy in `evolve/src/strategies/llm_guided.rs`: constructs mutation prompt from `MutationContext` (failing trace, score, criteria), bridges to async `JudgeClient::judge()` via `tokio::runtime::Handle::current().block_on()`, parses `JudgeVerdict.reason` as rewritten text. Handles `JudgeError` as `MutationError::JudgeUnavailable`
- [ ] T041 [US3] Create `evolve/src/strategies/mod.rs` re-exporting `LlmGuided`, `TemplateBased`, `Ablation`
- [ ] T042 [US3] Implement candidate deduplication helper in `evolve/src/mutate.rs`: `deduplicate(candidates: Vec<Candidate>, original: &str) -> Vec<Candidate>` — filter out candidates where `mutated_value == original`, then deduplicate remainder by SHA-256 ID
- [ ] T043 [US3] Verify all 6 mutation tests pass

---

## Phase 6: US4+US5 — Evaluate Candidates & Gate Acceptance (Priority: P1)

**Story goal**: Score candidates against baseline and accept/reject based on quality thresholds.

**Independent test**: Provide baseline + candidates with known scores, verify gate correctly accepts/rejects.

- [ ] T044 [US4] Write test `evolve/tests/gate.rs`: test `candidate_above_threshold_accepted` — candidate improves by 0.05 (threshold 0.01), no P1 regressions, verify `Accepted`
- [ ] T045 [US5] Write test `evolve/tests/gate.rs`: test `candidate_below_threshold_rejected` — candidate improves by 0.005, verify `BelowThreshold` rejection
- [ ] T046 [US5] Write test `evolve/tests/gate.rs`: test `p1_regression_rejected` — candidate improves aggregate but regresses a P1 case, verify `P1Regression` rejection
- [ ] T047 [US5] Write test `evolve/tests/gate.rs`: test `top_ranked_per_component` — two accepted candidates for same component, verify only top-ranked gets `Accepted`, other gets `AcceptedNotApplied`
- [ ] T048 [US5] Write test `evolve/tests/gate.rs`: test `custom_threshold_enforced` — threshold 0.10, candidate improves by 0.08, verify rejected
- [ ] T049 [US5] Write test `evolve/tests/gate.rs`: test `p2_case_regression_allowed` — candidate regresses a case with `metadata.priority = "P2"`, verify still accepted
- [ ] T050 [US4] Implement `CandidateEvaluator` in `evolve/src/evaluate.rs`: accepts `&OptimizationConfig`, `Arc<dyn AgentFactory>`. Method `evaluate_candidates(candidates, eval_set) -> Vec<CandidateResult>`. Uses `EvalRunner` with parallelism from config. For each candidate, wraps factory with `MutatingAgentFactory`. Add `#[cfg_attr(feature = "otel", tracing::instrument)]`
- [ ] T051 [US5] Implement `AcceptanceGate::new(threshold)` and `AcceptanceGate::evaluate()` in `evolve/src/gate.rs`: compare each candidate's aggregate score vs baseline. Check P1 regression (read `case.metadata["priority"]`, default P1). Rank accepted candidates by improvement descending. Apply per-component top-ranked-only rule (AcceptedNotApplied for others). Add `#[cfg_attr(feature = "otel", tracing::instrument)]`
- [ ] T052 [US4] Verify all 6 gate tests pass

---

## Phase 7: US6 — Persist Accepted Improvements (Priority: P1)

**Story goal**: Write improved configurations to versioned output directory with JSONL audit trail.

**Independent test**: Run persistence with known results, read back and verify directory structure and manifest contents.

- [ ] T053 [US6] Write test `evolve/tests/persist.rs`: test `manifest_contains_all_fields` — persist a cycle with 1 accepted and 1 rejected candidate, deserialize manifest, verify all fields non-null
- [ ] T054 [US6] Write test `evolve/tests/persist.rs`: test `output_directory_versioned` — persist two cycles, verify `cycle-0001-*` and `cycle-0002-*` subdirectories exist
- [ ] T055 [US6] Write test `evolve/tests/persist.rs`: test `no_config_written_when_all_rejected` — persist with all rejected, verify manifest exists but no config files written
- [ ] T056 [US6] Write test `evolve/tests/persist.rs`: test `manifest_jsonl_roundtrip` — write and read back manifest, verify serde round-trip
- [ ] T057 [US6] Implement `CyclePersister` in `evolve/src/persist.rs`: `new(output_root)`. Method `persist(cycle_number, acceptance_result, baseline) -> PathBuf`. Creates `cycle-{number:04}-{iso8601}/` directory. Writes `manifest.jsonl` with one `ManifestEntry` per candidate. Writes accepted config files (`system-prompt.md`, `tool-{name}.json`). Add `#[cfg_attr(feature = "otel", tracing::instrument)]`
- [ ] T058 [US6] Verify all 4 persistence tests pass

---

## Phase 8: US7 — Full Cycle Integration (Priority: P2)

**Story goal**: Orchestrate all phases end-to-end and support multi-cycle runs.

**Independent test**: Run `run_cycle()` with mock factory and small eval set, verify a known-weak prompt is improved.

- [ ] T059 [US7] Write test `evolve/tests/end_to_end.rs`: test `run_cycle_executes_all_phases` — mock factory with a prompt that fails one eval case, verify `CycleResult` contains baseline, weak points, and at least one candidate evaluated
- [ ] T060 [US7] Write test `evolve/tests/end_to_end.rs`: test `run_cycles_stops_on_no_improvement` — run with max 5 cycles, mock judge returns same text (no improvement), verify stops after 1 or 2 cycles
- [ ] T061 [US7] Write test `evolve/tests/end_to_end.rs`: test `budget_exhaustion_returns_partial_result` — set very low budget, verify `CycleStatus::BudgetExhausted` with phase name
- [ ] T062 [US7] Write test `evolve/tests/end_to_end.rs`: test `consecutive_cycles_chain_improvements` — mock judge that improves prompt, run 2 cycles, verify second cycle's baseline uses first cycle's accepted output. Assert `results[1].baseline.aggregate_score >= results[0].baseline.aggregate_score` (SC-008 monotonic non-decreasing)
- [ ] T063a [US7] Write test `evolve/tests/end_to_end.rs`: test `panic_strategy_caught_and_recorded` — register a mock `MutationStrategy` that panics in `mutate()`, run a cycle, verify `CycleResult` completes and manifest contains an entry with `MutationError::Panic` (SC-007)
- [ ] T063b [US7] Write test `evolve/tests/end_to_end.rs`: test `cycle_cost_matches_eval_costs` — run a cycle, compare `CycleResult.total_cost` against sum of baseline + candidate `EvalSetResult.summary.total_cost` values, assert within 5% (SC-005)
- [ ] T063 [US7] Implement `EvolutionRunner::run_cycle()` in `evolve/src/runner.rs`: orchestrates baseline → diagnose → mutate → evaluate → gate → persist. Propagates `CycleBudget` across phases. Returns `CycleResult`. Handles early exits (no weak points → `NoDiagnosis`, no candidates → `NoImprovements`, budget exhausted → `BudgetExhausted`). Add `#[cfg_attr(feature = "otel", tracing::instrument)]`
- [ ] T064 [US7] Implement `EvolutionRunner::run_cycles(max)` in `evolve/src/runner.rs`: loop calling `run_cycle()`, update target with accepted improvements after each cycle, stop early if `NoImprovements` or `NoDiagnosis`
- [ ] T065 [US7] Wire panic isolation for mutation strategies in `run_cycle()`: wrap each `strategy.mutate()` call in `std::panic::catch_unwind()`, convert panics to `MutationError::Panic`
- [ ] T066 [US7] Verify all 6 end-to-end tests pass

---

## Phase 9: US8 — History Inspection (Priority: P3)

**Story goal**: Load and iterate over cycle manifests from the output directory.

**Independent test**: Create manifests for 3 cycles, load them, verify ordering and round-trip.

- [ ] T067 [US8] Write test in `evolve/tests/persist.rs`: test `load_manifests_ordered_by_cycle` — write 3 cycles, call `CyclePersister::load_history()`, verify 3 entries ordered by cycle number
- [ ] T068 [US8] Implement `CyclePersister::load_history(output_root) -> Vec<(u32, Vec<ManifestEntry>)>` in `evolve/src/persist.rs`: scan `cycle-*` subdirectories, parse `manifest.jsonl` from each, return sorted by cycle number
- [ ] T069 [US8] Verify history test passes

---

## Phase 10: Polish & Cross-Cutting Concerns

**Purpose**: Final cleanup, documentation, and validation

- [ ] T070 Run `cargo clippy --workspace -- -D warnings` and fix any warnings in `evolve/` crate
- [ ] T071 Run `cargo test --workspace` and verify all tests pass including existing crates (no regressions)
- [ ] T072 Verify `cargo build --workspace` compiles with default features (evolve feature disabled) — zero impact on existing crates
- [ ] T073 Verify `cargo build -p swink-agent-evolve` compiles independently
- [ ] T074 Verify `cargo build -p swink-agent-evolve --features otel` compiles with observability spans
- [ ] T075 Update `evolve/src/lib.rs` module-level doc comment with one-paragraph crate description

---

## Dependencies

```text
Phase 1 (Setup) ──────► Phase 2 (Foundation) ──────┬──► Phase 3 (US1: Baseline)
                                                    │         │
                                                    │         ▼
                                                    ├──► Phase 4 (US2: Diagnose)
                                                    │         │
                                                    │         ▼
                                                    ├──► Phase 5 (US3: Mutate)
                                                    │         │
                                                    │         ▼
                                                    └──► Phase 6 (US4+5: Evaluate+Gate)
                                                              │
                                                              ▼
                                                    Phase 7 (US6: Persist)
                                                              │
                                                              ▼
                                                    Phase 8 (US7: Full Cycle)
                                                              │
                                                              ▼
                                                    Phase 9 (US8: History)
                                                              │
                                                              ▼
                                                    Phase 10 (Polish)
```

**Key constraints**:
- US1 (baseline) must complete before US2 (diagnose needs `BaselineSnapshot`)
- US2 (diagnose) must complete before US3 (mutate needs `WeakPoint`)
- US3 (mutate) must complete before US4+5 (evaluate+gate needs `Candidate`)
- US4+5 must complete before US6 (persist needs `AcceptanceResult`)
- US6 must complete before US7 (full cycle needs all phases)
- US8 (history) only needs US6 (persist)
- Phase 2 types are parallelizable (all [P] tasks)

## Parallel Execution Opportunities

**Phase 2**: All T004–T017 can run in parallel (independent files, no cross-dependencies)

**Phase 5**: T038, T039, T040 can run in parallel (independent strategy files)

**Phase 6**: T050 (evaluate) and T051 (gate) can run in parallel (independent modules)

**Phase 7**: All test tasks (T053–T056) can run before implementation (TDD)

## Implementation Strategy

**MVP**: Phase 1 + Phase 2 + Phase 3 (US1: Baseline). This validates eval crate integration and proves the `MutatingAgentFactory` approach works. Delivers `EvolutionRunner::baseline()` as a standalone capability.

**Incremental delivery**:
1. MVP: Baseline evaluation (US1)
2. Add diagnosis (US2) — can now identify weak points from any eval run
3. Add mutations (US3) — can now generate candidates
4. Add evaluation + gating (US4+5) — can now accept/reject candidates
5. Add persistence (US6) — full single-cycle capability
6. Add orchestration (US7) — multi-cycle optimization loop
7. Add history (US8) — retrospective analysis
