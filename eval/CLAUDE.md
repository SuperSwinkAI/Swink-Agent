# CLAUDE.md — Evaluation Framework

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
