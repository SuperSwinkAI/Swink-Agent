# Implementation Plan: Evals: Advanced Features

**Branch**: `043-evals-adv-features` | **Date**: 2026-04-21 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/043-evals-adv-features/spec.md`

## Summary

Layer a production-ready LLM-as-judge and advanced evaluation surface on top of the `swink-agent-eval` crate shipped by specs 023/024. A new workspace crate `swink-agent-eval-judges` hosts per-provider `JudgeClient` implementations behind feature flags. The existing `eval` crate is extended with a versioned prompt-template registry, 24 judge-backed and deterministic evaluators across seven families (quality, safety, RAG, agent, structured, simple, code), multi-turn simulation (`ActorSimulator`/`ToolSimulator`), auto-generation (`ExperimentGenerator`/`TopicPlanner`), OTel/Langfuse/OpenSearch/CloudWatch trace ingestion, runner upgrades (parallelism, `num_runs`, cached invocations, cancellation), plain-text/JSON/Markdown/HTML reporters, a LangSmith exporter, and a `swink-eval` CLI binary. Per FR-047, the default build adds no new mandatory dependencies — every new surface is gated behind an opt-in feature.

## Technical Context

**Language/Version**: Rust latest stable (workspace pins `1.95` MSRV), edition 2024
**Primary Dependencies (existing)**: `swink-agent` (core types, `CancellationToken`, `AgentEvent`), `swink-agent-eval` (023/024 — `Evaluator`, `EvalCase`, `EvalSet`, `Invocation`, `EvalRunner`, `FsEvalStore`, `GateConfig`, `Score`, `Verdict`, `JudgeClient`, `JudgeVerdict`, `JudgeError`, `MockJudge`), `serde`/`serde_json`, `tokio`/`tokio-util` (async + cancellation), `futures`, `tracing`, `uuid` (v4 + v5), `regex`, `sha2` (content-hash cache keys + UUID v5 namespaces), `thiserror`
**Primary Dependencies (new, all feature-gated)**:
  - `minijinja` (prompt templating with named variables and compile-time checks) — judge-core feature
  - `backoff` (exponential-backoff retry) — judge-core feature
  - `strsim` (Levenshtein) — `evaluator-simple` feature
  - `jsonschema` (deterministic schema validation) — `evaluator-structured` feature
  - `opentelemetry` + `opentelemetry-sdk` (OTel SDK for `EvalsTelemetry`) — `telemetry` feature
  - `opentelemetry-otlp` — `trace-otlp` feature
  - `opentelemetry-stdout` (optional dev reporter) — `telemetry` feature
  - Provider adapters: `swink-agent-adapters-{anthropic,openai,gemini,bedrock,mistral,azure,xai,ollama,proxy}` — per-provider `judge-<name>` features in `eval-judges`
  - `handlebars` or `askama` (HTML templating) — `html-report` feature
  - `libc` (POSIX rlimit/setrlimit on Unix) — `evaluator-sandbox` feature (Unix-only)
  - `reqwest` (LangSmith HTTP) — `langsmith` feature
  - `clap` (CLI parsing) — `cli` feature / binary
  - `dev-dependencies only`: `wiremock` (HTTP test doubles for judge clients), `opentelemetry-stdout` or in-memory exporter for trace-ingestion tests
**Storage**: Filesystem for `LocalFileTaskResultStore` (extends `FsEvalStore` pattern from 024). No database dependency.
**Testing**: `cargo test -p swink-agent-eval` and `cargo test -p swink-agent-eval-judges` using `MockJudge` + `wiremock`. A `live-judges` feature gates a small canary suite that hits real provider endpoints when explicit env-vars are set.
**Target Platform**: Linux, macOS, Windows — with `SandboxedExecutionEvaluator` deliberately Unix-only (Windows fails fast with `EvaluatorError::UnsupportedPlatform`).
**Project Type**: Library crates (`swink-agent-eval` extended; `swink-agent-eval-judges` new) + a CLI binary target (`swink-eval`) hosted in the `eval` crate behind a `cli` feature.
**Performance Goals**: SC-002 — 20-case/parallelism-4/fast-provider suite completes in under 2× single-case wall-clock; SC-003 — prompt-only re-run reuses cached invocations and skips all agent calls.
**Constraints**:
  - `#![forbid(unsafe_code)]` across both crates (FR-049).
  - Default build MUST add no new mandatory deps beyond what spec 023 pulled in (FR-047, SC-009).
  - Both async (`evaluate_async`) and blocking (`evaluate`) entry points on every evaluator, correct inside and outside a Tokio runtime (FR-048).
  - Panic isolation at registry boundary (FR-021) — every new evaluator/simulator/generator/reporter catches panics and converts to `Score::fail()` with diagnostic context.
**Scale/Scope**:
  - 50 functional requirements (FR-001 through FR-043 and FR-045 through FR-050; FR-044 was removed during clarification).
  - 24 evaluators across 7 families (spec §Key Entities + Q1 clarification splits pairs).
  - 9 provider `JudgeClient` implementations, each behind its own feature flag in `eval-judges`.
  - 4+ trace providers (in-memory always-on; OTLP-HTTP, Langfuse, OpenSearch, CloudWatch feature-gated).
  - 5 reporters (console/json/md always-on; html + langsmith feature-gated).
  - New workspace crate raises member count from 14 → 15.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

No formal `constitution.md` exists in this repository. Prior plans (042, 023, 024) apply the following implicit workspace principles, and this plan preserves them:

| Principle | Status | Notes |
|---|---|---|
| I. Library-First | **PASS** | `swink-agent-eval-judges` is a new independent crate depending on `swink-agent-eval` + per-provider adapter crates. `swink-agent-eval` extensions are additive and feature-gated. Neither crate takes a reverse dep from core. |
| II. Test-Driven | **PASS** | Per FR-050, the default test suite uses `MockJudge` and HTTP test doubles — no live LLM calls. A `live-judges` feature opts into a tiny canary suite. Every new evaluator and simulator ships with contract tests before implementation. |
| III. Efficiency & Performance | **PASS** | Runner gains structured parallelism (bounded semaphore); `EvaluationDataStore` avoids agent re-invocations; `JudgeCache` avoids duplicate prompt dispatches; `JudgeClient` supports request batching. SC-002/SC-003 measurable. |
| IV. Leverage the Ecosystem | **PASS** | `minijinja`, `backoff`, `strsim`, `jsonschema`, `opentelemetry`, `handlebars`/`askama`, `clap`, `libc`, `wiremock` — all well-maintained crates. No hand-rolled alternatives. |
| V. Provider Agnosticism | **PASS** | `JudgeClient` trait is the only interface evaluators depend on. Per-provider implementations live in `eval-judges` behind feature flags and wrap adapter crates without polluting them. |
| VI. Safety & Correctness | **PASS** | `#![forbid(unsafe_code)]` on both crate roots; sandbox uses POSIX rlimit/setrlimit via `libc` (the only FFI surface), localized to `evaluators::code::sandbox::posix` and explicitly permitted by FR-049's carve-out (which pins the `cfg(target_family = "unix")` and `// SAFETY:` requirements). SSRF guards on `Attachment::Url`. Panic isolation per FR-021, which also mandates score clamping to `[0.0, 1.0]`. Cancellation propagates cooperatively per FR-040. |
| Crate count (14 → 15) | **JUSTIFIED** | `eval-judges` has per-provider adapter deps that don't belong in the pure `eval` crate; adding them to `eval` would drag every adapter into `eval`'s default-build transitive set even behind features (Cargo feature unification bugs). A separate crate lets consumers enable only the providers they need without pulling in others' adapter crates as dev-deps. |
| MSRV | **PASS** | Stable Rust, edition 2024. All new deps tested against 1.95. |

**Gate result: ALL PASS** — no unjustified violations.

## Project Structure

### Documentation (this feature)

```text
specs/043-evals-adv-features/
├── spec.md              # Feature specification (clarified 2026-04-21)
├── plan.md              # This file
├── research.md          # Phase 0: architectural decisions and dep choices
├── data-model.md        # Phase 1: entity relationships and type signatures
├── quickstart.md        # Phase 1: end-to-end usage walkthroughs
├── contracts/           # Phase 1: public API contracts
│   └── public-api.md
├── checklists/
│   └── requirements.md  # (already present)
└── tasks.md             # Phase 2 output (generated by /speckit.tasks)
```

### Source Code (repository root)

```text
eval/                                     # existing crate, extended
├── Cargo.toml                            # adds feature flags + optional deps
├── src/
│   ├── lib.rs                            # new public re-exports for added surfaces
│   ├── types.rs                          # extended: EvalCase fields per FR-043, Attachment enum
│   ├── evaluator.rs                      # extended: EvaluatorRegistry supports panic isolation for new families
│   ├── runner.rs                         # extended: parallelism, num_runs, cancellation, initial_session_file
│   ├── score.rs                          # unchanged (inherited from 023/024)
│   ├── prompt/
│   │   ├── mod.rs                        # JudgePromptTemplate trait, registry
│   │   ├── templates.rs                  # Built-in templates (quality/safety/RAG/agent/structured/code)
│   │   └── version.rs                    # Version suffix helpers (_v0, _v1)
│   ├── judge/
│   │   ├── mod.rs                        # JudgeRegistry, JudgeCache, retry policy
│   │   └── cache.rs                      # In-memory LRU + optional disk-backed cache
│   ├── aggregator/
│   │   └── mod.rs                        # Aggregator trait + Average/AllPass/AnyPass/Weighted built-ins
│   ├── evaluators/
│   │   ├── mod.rs                        # Family module re-exports
│   │   ├── quality.rs                    # Helpfulness, Correctness, Conciseness, Coherence,
│   │   │                                 #   ResponseRelevance, Hallucination, Faithfulness,
│   │   │                                 #   PlanAdherence, Laziness, GoalSuccessRate
│   │   ├── safety.rs                     # Harmfulness, Toxicity, Fairness, PIILeakage,
│   │   │                                 #   PromptInjection, CodeInjection
│   │   ├── rag.rs                        # RAGGroundedness, RAGRetrievalRelevance,
│   │   │                                 #   RAGHelpfulness, EmbeddingSimilarity (+ Embedder trait)
│   │   ├── agent.rs                      # TrajectoryAccuracy, TaskCompletion, UserSatisfaction,
│   │   │                                 #   AgentTone, KnowledgeRetention, LanguageDetection,
│   │   │                                 #   PerceivedError, Interactions
│   │   ├── structured.rs                 # JsonMatch (per-key rubrics), JsonSchema
│   │   ├── simple.rs                     # ExactMatch, LevenshteinDistance
│   │   ├── code/
│   │   │   ├── mod.rs
│   │   │   ├── cargo_check.rs
│   │   │   ├── clippy.rs
│   │   │   ├── extractor.rs              # CodeExtractor (markdown-fence / LLM / regex)
│   │   │   ├── llm_judge.rs              # CodeLlmJudgeEvaluator
│   │   │   └── sandbox.rs                # SandboxedExecutionEvaluator (Unix only; Windows stub)
│   │   └── multimodal.rs                 # Image-safety evaluators (behind `multimodal` feature)
│   ├── simulation/
│   │   ├── mod.rs                        # (behind `simulation` feature)
│   │   ├── actor.rs                      # ActorSimulator, ActorProfile
│   │   ├── tool.rs                       # ToolSimulator, StateRegistry, StateBucket
│   │   └── orchestrator.rs               # run_multiturn_simulation
│   ├── generation/
│   │   ├── mod.rs                        # (behind `generation` feature)
│   │   ├── experiment.rs                 # ExperimentGenerator
│   │   └── topic.rs                      # TopicPlanner
│   ├── trace/
│   │   ├── mod.rs                        # (behind `trace-ingest` feature)
│   │   ├── provider.rs                   # TraceProvider trait + OtelInMemoryTraceProvider
│   │   ├── mapper.rs                     # SessionMapper trait,
│   │   │                                 #   OpenInferenceSessionMapper,
│   │   │                                 #   LangChainSessionMapper,
│   │   │                                 #   OtelGenAiSessionMapper + GenAIConventionVersion
│   │   ├── extractor.rs                  # TraceExtractor, EvaluationLevel,
│   │   │                                 #   SwarmExtractor (spec 040),
│   │   │                                 #   GraphExtractor (spec 039)
│   │   ├── otlp.rs                       # OTLP-HTTP provider (feature `trace-otlp`)
│   │   ├── langfuse.rs                   # (feature `trace-langfuse`)
│   │   ├── opensearch.rs                 # (feature `trace-opensearch`)
│   │   └── cloudwatch.rs                 # (feature `trace-cloudwatch`)
│   ├── telemetry.rs                      # EvalsTelemetry (feature `telemetry`)
│   ├── cache.rs                          # EvaluationDataStore + LocalFileTaskResultStore
│   ├── url_filter.rs                     # UrlFilter trait + DefaultUrlFilter (always-on; used by attachment materialization)
│   ├── report/
│   │   ├── mod.rs                        # Reporter trait + Report types
│   │   ├── console.rs                    # Plain-text ConsoleReporter (always-on)
│   │   ├── json.rs                       # JsonReporter (always-on)
│   │   ├── markdown.rs                   # MarkdownReporter (always-on)
│   │   ├── html.rs                       # HtmlReporter (feature `html-report`)
│   │   └── langsmith.rs                  # LangSmithExporter (feature `langsmith`)
│   ├── ci/
│   │   └── templates/                    # Shipped GitHub Actions YAML templates (static files)
│   └── bin/
│       └── swink_eval.rs                 # `swink-eval` CLI binary (feature `cli`)
└── tests/
    ├── common/mod.rs
    ├── judge_registry_test.rs
    ├── prompt_template_test.rs
    ├── evaluators_quality_test.rs
    ├── evaluators_safety_test.rs
    ├── evaluators_rag_test.rs
    ├── evaluators_agent_test.rs
    ├── evaluators_structured_test.rs
    ├── evaluators_simple_test.rs
    ├── evaluators_code_test.rs
    ├── simulation_test.rs
    ├── generation_test.rs
    ├── trace_ingest_test.rs
    ├── runner_parallelism_test.rs
    ├── runner_num_runs_test.rs
    ├── cache_test.rs
    ├── reporter_test.rs
    ├── telemetry_test.rs
    └── cli_test.rs

eval-judges/                              # NEW workspace crate
├── Cargo.toml                            # per-provider feature flags
├── src/
│   ├── lib.rs                            # Public API + feature-gated re-exports
│   ├── client.rs                         # Shared retry + backoff wiring used by each provider
│   ├── anthropic.rs                      # AnthropicJudgeClient (feature `anthropic`)
│   ├── openai.rs                         # (feature `openai`)
│   ├── bedrock.rs                        # (feature `bedrock`)
│   ├── gemini.rs                         # (feature `gemini`)
│   ├── mistral.rs                        # (feature `mistral`)
│   ├── azure.rs                          # (feature `azure`)
│   ├── xai.rs                            # (feature `xai`)
│   ├── ollama.rs                         # (feature `ollama`)
│   └── proxy.rs                          # (feature `proxy`)
└── tests/
    ├── common/mod.rs                     # Shared wiremock fixtures
    ├── anthropic_test.rs                 # (gated by cfg(feature = "anthropic"))
    ├── openai_test.rs
    └── ...                               # one per provider
```

**Structure Decision**:

1. **Extend `eval` rather than fragment.** Prompt registry, evaluators, simulation, generation, trace ingestion, telemetry, cache, reporters, and the CLI binary all live in `swink-agent-eval` behind feature flags. This keeps the consumer surface coherent (one crate to depend on) and matches the precedent set by `swink-agent-eval` already absorbing 023 + 024.
2. **New `eval-judges` crate for provider judge clients only.** The spec's own Assumptions line 349 mandates this, and the rationale (adapter-dep isolation, per-provider feature flags without polluting `eval`'s default transitive set) is sound. Each provider feature enables exactly one `<Provider>JudgeClient` impl + its adapter dependency.
3. **`swink-eval` CLI as a binary target in `eval`.** Behind a `cli` feature. Avoids a separate crate for ~500 LoC of `clap` wiring. `cargo install swink-agent-eval --features cli` gives users the binary.
4. **Feature-flag naming convention** (pinned here; see R-010 in research.md):
   - On `eval`: `judge-core`, `evaluator-quality`, `evaluator-safety`, `evaluator-rag`, `evaluator-agent`, `evaluator-structured`, `evaluator-simple`, `evaluator-code`, `evaluator-sandbox`, `multimodal`, `simulation`, `generation`, `trace-ingest`, `trace-otlp`, `trace-langfuse`, `trace-opensearch`, `trace-cloudwatch`, `telemetry`, `html-report`, `langsmith`, `cli`, `live-judges`.
   - On `eval-judges`: `anthropic`, `openai`, `bedrock`, `gemini`, `mistral`, `azure`, `xai`, `ollama`, `proxy`. Each enables its own adapter dep plus `eval/judge-core`.
   - Meta-feature `all-judges` on `eval-judges` and `all-evaluators` on `eval` for docs.rs and integration testing.

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|---|---|---|
| Adding a 15th workspace crate (`eval-judges`) | Per-provider `JudgeClient` implementations wrap adapter crates; placing them in `eval` would make the default build's transitive-dep graph include adapter internals through Cargo feature unification in transitive dev-dep contexts | Combining with `eval` causes feature-unification pollution where enabling any eval consumer accidentally pulls adapter deps into other workspace members' builds; spec 043 Assumptions line 349 explicitly pins this decomposition |
| `cli` binary target inside `eval` (not a separate crate) | CLI is ~500 LoC of `clap` wiring over `eval`'s public API; a separate crate would be a pure consumer with no independent logic | A standalone `eval-cli` crate would double the workspace-member count increase (14 → 16) without architectural benefit; binary target pattern is established (`xtask`) |
| 22 new features on `eval` | Every spec scope item (11) splits into always-on core + multiple opt-in surfaces (evaluator families, trace backends, reporters, CLI); per FR-047 nothing can be mandatory | A monolithic always-on build violates FR-047 and SC-009 explicitly — the default build of `swink-agent-eval` MUST NOT add new mandatory dependencies beyond what spec 023 requires |
