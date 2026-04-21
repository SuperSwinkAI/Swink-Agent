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
- `EvaluatorRegistry::evaluate()` is the panic-isolation boundary for eval scoring. Wrap each evaluator call in `catch_unwind(AssertUnwindSafe(...))`, record a failing metric for the panicking evaluator, and keep running the remaining evaluators/cases.
