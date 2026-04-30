# AGENTS.md — Evaluation Framework

## Scope

`eval/` — Trajectory tracing, golden path verification, response matching, cost/latency governance.

## Key Facts

- `Evaluator` returns `Option<EvalMetricResult>` — `None` = not applicable.
- Built-ins via `EvaluatorRegistry::with_defaults()`: `TrajectoryMatcher`, `BudgetEvaluator`, `ResponseMatcher`, `EfficiencyEvaluator`.
- `TrajectoryCollector` consumes `AgentEvent` streams. `AgentFactory` is sync.
- `FsEvalStore` layout: `{dir}/sets/{id}.json`, `{dir}/results/{eval_set_id}/{timestamp}.json`.
- `run_set()` catches per-case errors as `Verdict::Fail` — does NOT abort the suite.
- `BudgetConstraints` → `to_policies()` for enforcement; `TrajectoryCollector` only drains events.

## Key Invariants

- Code-family evaluators invoke helpers at runtime — their deps (e.g. `tempfile`) go in `[dependencies]`, not `[dev-dependencies]`.
- Unsafe code is denied everywhere in `eval` except the Unix-only `evaluators::code::sandbox::posix` rlimit FFI layer. The crate root uses `#![deny(unsafe_code)]`, not `forbid`, solely because `forbid` cannot be relaxed by that documented nested carve-out.
- `FsEvalStore` validates set IDs (reject empty, `..`, NUL, separators) and persists via `atomic_fs`.
- Evaluator panics isolated via `tokio::spawn`/`catch_unwind` → failing metric, not suite abort.
- Empty evaluator output = failed case (`no_applicable_evaluators`), not vacuous pass.
- Multi-run cancellation emits `"cancelled"` failing metric. `EvalRunner` cancels token on every exit path.
- `TrajectoryCollector` backfills missing tool calls from `TurnEnd.assistant_message` (pre-dispatch skips have no start event).
- `JudgeClient` uses boxed-future return (no `async-trait`). Judge-backed evaluators build prompts through shared `build_prompt_context()`.
- `Attachment::Url` materialization revalidates every redirect hop against HTTPS policy and `UrlFilter`.
- Multimodal-only helpers must be feature-gated together with their tests to avoid dead-code warnings.
- `EvalCase` extensions stay serde-backward-compatible. Load-time validation rejects duplicate IDs and malformed attachments.
