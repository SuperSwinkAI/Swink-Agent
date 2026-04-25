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

- Code-family evaluators execute from the crate's runtime library surface, not just tests, so helper crates they invoke at evaluation time (for example `tempfile` for `CargoCheckEvaluator` / `ClippyEvaluator`) must live in `[dependencies]`, not only `[dev-dependencies]`.
- `FsEvalStore` must validate eval set IDs before any path join. Reject empty IDs, `.`/`..`, NUL, and both `/` and `\` separators even on non-Windows hosts so logical identifiers cannot escape `sets/` or `results/` when tests or artifacts move across platforms.
- `FsEvalStore` set/result persistence must go through `swink_agent::atomic_fs` helpers rather than direct `fs::write`, so interrupted rewrites never leave partial JSON or clobber the last good file.
- `BudgetConstraints` no longer enforce limits inside `TrajectoryCollector`. Convert them with `to_policies()` and attach the returned `BudgetPolicy` / `MaxTurnsPolicy` in `AgentFactory`; `TrajectoryCollector` only drains the event stream.
- `EvaluatorRegistry::evaluate()` is the panic-isolation boundary for eval scoring. Route each evaluator call through `eval::evaluator::isolate_panic(...)`: use `tokio::spawn(...).await` when a multi-thread runtime is already active so panics surface as `JoinError::is_panic()`, and fall back to inline `catch_unwind` only for plain sync callers with no runtime. Either way, record a failing metric for the panicking evaluator and keep running the remaining evaluators/cases.
- `EnvironmentStateEvaluator` is safe to register in `EvaluatorRegistry::with_defaults()`: it remains inert unless both `expected_environment_state` and `state_capture` are present, and any capture panic must be converted into `Score::fail()` rather than escaping.
- Cross-cutting panic-isolation coverage for semantic evaluators must run on a multi-thread Tokio runtime: `SemanticToolSelectionEvaluator` / `SemanticToolParameterEvaluator` bridge async judge calls from the sync `Evaluator` API via `block_in_place`, and a panicking `JudgeClient` must surface as registry-level `evaluator panicked` failure metrics rather than aborting `EvalRunner`.
- Spec 023 quickstarts that demonstrate US5/US6/US7 should use `EvaluatorRegistry::with_defaults_and_judge(Arc::new(MockJudge::always_pass()))` plus an inline `state_capture` closure. Both `state_capture` and `ResponseCriteria::Custom` are programmatic-only surfaces, so YAML/JSON fixture examples under-document the real integration path.
- Deterministic eval session IDs must hash a canonical `CaseFingerprint`, not raw JSON insertion order or closure addresses. Sort object keys recursively and treat programmatic-only closures (`state_capture`, `ResponseCriteria::Custom` bodies) as stable markers so the same logical case yields the same UUID across re-runs.
- `DefaultUrlFilter` is intentionally SSRF-focused, not transport-policy-focused: it blocks localhost, private/link-local IPs, and known cloud metadata hosts, but leaves HTTPS enforcement to later attachment/materialization layers so callers can compose stricter fetch policies on top.
- `Attachment::Url` materialization must revalidate every redirect hop against both HTTPS-only transport policy and the active `UrlFilter`; validating only the initial URL leaves an SSRF gap because `reqwest` otherwise follows redirects automatically.
- US1 acceptance coverage should import evaluators from the crate root in `eval/tests/us1_end_to_end_test.rs` instead of module paths. That single test guards both registry wiring and `eval/src/lib.rs` public re-exports, so T088/T089 regress together.
- `TrajectoryCollector` cannot rely solely on `ToolExecutionStart` for tool intent. Pre-dispatch skips and approval rejections emit terminal error results without a start event, so turn finalization must backfill missing tool calls from `TurnEnd.assistant_message` while still preferring any observed execution-start arguments.
- `EvalCase` spec-043 extensions (`expected_assertion`, `expected_interactions`, `few_shot_examples`, `session_id`) must stay serde-backward-compatible and explicit in test fixtures. Load-time validation rejects duplicate case IDs, blank assertion/interaction/example fields, and attachment declarations that are obviously malformed (absolute or parent-traversal paths, non-HTTPS URLs, unsupported MIME) before runtime materialization.
- Shared wiremock judge-response builders live in [`eval/tests/common/judge_fixtures.rs`](C:/Users/remote/.codex/worktrees/d410/Swink-Agent/eval/tests/common/judge_fixtures.rs) and [`eval-judges/tests/common/mod.rs`](C:/Users/remote/.codex/worktrees/d410/Swink-Agent/eval-judges/tests/common/mod.rs). New judge/provider tests should reuse those helpers so success, 429/retry, malformed JSON, and delayed-cancellation cases stay consistent across crates.
- Judge-backed evaluators must build prompt render context through the shared `build_prompt_context()` helper in [`eval/src/evaluators/mod.rs`](C:/Users/remote/.codex/worktrees/d83f/Swink-Agent/eval/src/evaluators/mod.rs), not ad hoc per-family helpers. That helper is the only place that currently merges evaluator-level `few_shot_examples`, `system_prompt`, `output_schema`, `use_reasoning`, and `feedback_key` overrides into the template-visible context, so bypassing it silently drops US3 prompt overrides.
- LangSmith export cannot recover evaluator `feedback_key` from `EvalMetricResult` unless judge-backed dispatch records it explicitly alongside `prompt_version` in the structured detail JSON lines. Preserve exporter-facing metadata in `Detail` entries instead of trying to reverse-engineer it from evaluator names or free-form `details` text after the fact.
- Multimodal-only attachment helpers in [`eval/src/types.rs`](C:/Users/remote/.codex/worktrees/e357/Swink-Agent/eval/src/types.rs) must be gated together with their unit tests. Leaving redirect-validation helpers compiled in non-`multimodal` builds trips the workspace `-D warnings` / dead-code gate even though the runtime path is feature-disabled.
- `include_str!` only validates that bundled CI template files exist. If a spec acceptance point requires those templates to remain valid YAML, keep a parser-backed unit test in [`eval/src/ci/mod.rs`](C:/Users/remote/.codex/worktrees/6dfe/Swink-Agent/eval/src/ci/mod.rs) so malformed template edits fail fast in CI instead of shipping broken scaffolds.
- `JudgeClient` is part of the always-on spec-023 surface, so it cannot rely on `async-trait` without violating the spec-043 SC-009 default-dependency baseline. Keep it object-safe with a boxed-future return type; reserve proc-macro async trait helpers for opt-in feature paths only.
