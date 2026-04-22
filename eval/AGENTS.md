# AGENTS.md — Evaluation Framework

## Scope

`eval/` — Trajectory tracing, golden path verification, response matching, cost/latency governance.

## Key Facts

- `Evaluator` trait returns `Option<EvalMetricResult>` — `None` = not applicable.
- Built-ins: `TrajectoryMatcher`, `BudgetEvaluator`, `ResponseMatcher`, `EfficiencyEvaluator` (via `EvaluatorRegistry::with_defaults()`).
- `TrajectoryCollector` consumes `AgentEvent` streams, not agents directly.
- `AgentFactory` is sync because `Agent::prompt_stream()` is sync.
- `FsEvalStore` layout: `{dir}/sets/{id}.json`, `{dir}/results/{eval_set_id}/{timestamp}.json`.
- `ResponseCriteria::Custom` is `#[serde(skip)]` — set programmatically only.
- `run_set()` catches per-case agent errors and records them as failed `EvalCaseResult`s with `Verdict::Fail` and an "error" metric — it does NOT abort the suite (FR-003).
- Runner tests require `swink-agent` `test-helpers` feature for `MockStreamFn` — declared in `eval/Cargo.toml` dev-dependencies.

## Lessons Learned

- `FsEvalStore` must validate eval set IDs before any path join. Reject empty IDs, `.`/`..`, NUL, and both `/` and `\` separators even on non-Windows hosts so logical identifiers cannot escape `sets/` or `results/` when tests or artifacts move across platforms.
- `FsEvalStore` set/result persistence must go through `swink_agent::atomic_fs` helpers rather than direct `fs::write`, so interrupted rewrites never leave partial JSON or clobber the last good file.
- `BudgetConstraints` no longer enforce limits inside `TrajectoryCollector`. Convert them with `to_policies()` and attach the returned `BudgetPolicy` / `MaxTurnsPolicy` in `AgentFactory`; `TrajectoryCollector` only drains the event stream.
- `EvaluatorRegistry::evaluate()` is the panic-isolation boundary for eval scoring. Wrap each evaluator call in `catch_unwind(AssertUnwindSafe(...))`, record a failing metric for the panicking evaluator, and keep running the remaining evaluators/cases.
- `EnvironmentStateEvaluator` is safe to register in `EvaluatorRegistry::with_defaults()`: it remains inert unless both `expected_environment_state` and `state_capture` are present, and any capture panic must be converted into `Score::fail()` rather than escaping.
- Cross-cutting panic-isolation coverage for semantic evaluators must run on a multi-thread Tokio runtime: `SemanticToolSelectionEvaluator` / `SemanticToolParameterEvaluator` bridge async judge calls from the sync `Evaluator` API via `block_in_place`, and a panicking `JudgeClient` must surface as registry-level `evaluator panicked` failure metrics rather than aborting `EvalRunner`.
- Spec 023 quickstarts that demonstrate US5/US6/US7 should use `EvaluatorRegistry::with_defaults_and_judge(Arc::new(MockJudge::always_pass()))` plus an inline `state_capture` closure. Both `state_capture` and `ResponseCriteria::Custom` are programmatic-only surfaces, so YAML/JSON fixture examples under-document the real integration path.
- Deterministic eval session IDs must hash a canonical `CaseFingerprint`, not raw JSON insertion order or closure addresses. Sort object keys recursively and treat programmatic-only closures (`state_capture`, `ResponseCriteria::Custom` bodies) as stable markers so the same logical case yields the same UUID across re-runs.
