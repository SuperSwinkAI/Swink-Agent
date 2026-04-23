# Tasks: Evals: Advanced Features

**Input**: Design documents from `/specs/043-evals-adv-features/`
**Prerequisites**: spec.md, plan.md, research.md, data-model.md, contracts/public-api.md, quickstart.md

**Tests**: Included — FR-050 mandates mock/test-double-only default suite; the implicit Constitution II (Test-Driven) applies as in prior specs. Tests precede implementation within each story.

**Organization**: Tasks grouped by user story. MVP = US1, US2, US3 (all P1). US4–US8 are P2; US9 is P3. Nothing in the MVP requires work outside the MVP stories.

## Format: `- [ ] [TaskID] [P?] [Story?] Description with file path`

- **[P]**: Parallelizable — different files, no unfinished dependency.
- **[Story]**: `[US1]`–`[US9]` on story-phase tasks. No label on Setup / Foundational / Polish.
- File paths are absolute unless a repo-relative path is obvious.

## Path Conventions

- `swink-agent-eval` extensions live under `eval/src/`.
- New `swink-agent-eval-judges` crate lives at `eval-judges/` at the workspace root.
- CLI binary: `eval/src/bin/swink_eval.rs`.
- CI templates: `eval/src/ci/templates/`.
- Shared integration tests: `eval/tests/` and `eval-judges/tests/`.

---

## Phase 1: Setup

**Purpose**: Create the new `eval-judges` crate, wire workspace, declare feature flags and new deps. No logic yet.

- [ ] T001 Create `eval-judges/` directory with `src/` and `tests/common/` subdirectories
- [ ] T002 Create `eval-judges/Cargo.toml`: package `swink-agent-eval-judges`, workspace inheritance for version/edition/rust-version/license/repository, `#![forbid(unsafe_code)]`, dependencies (`swink-agent-eval` path dep, `async-trait`, `tokio`, `backon`, `tracing`, `thiserror`, `serde`, `serde_json`), optional per-provider adapter deps (`swink-agent-adapters-anthropic`, `-openai`, `-bedrock`, `-gemini`, `-mistral`, `-azure`, `-xai`, `-ollama`, `-proxy`), feature flags (`anthropic`, `openai`, `bedrock`, `gemini`, `mistral`, `azure`, `xai`, `ollama`, `proxy`, `all-judges`, `live-judges`) each enabling its adapter dep plus `swink-agent-eval/judge-core`
- [ ] T003 Add `"eval-judges"` to `[workspace] members` in root `Cargo.toml`
- [ ] T004 Add new workspace-level dependencies per research.md §R-025 to root `Cargo.toml` `[workspace.dependencies]`: `minijinja` 2, `backon` 1, `strsim` 0.12, `jsonschema` 0.30, `opentelemetry` 0.31, `opentelemetry-sdk` 0.31, `opentelemetry-otlp` 0.31, `askama` 0.13, `clap` 4, `libc` 0.2
- [ ] T005 Update `eval/Cargo.toml`: add optional deps (`minijinja`, `backon`, `strsim`, `jsonschema`, `opentelemetry`, `opentelemetry-sdk`, `opentelemetry-otlp`, `opentelemetry-stdout`, `askama`, `clap`, `libc`, `reqwest`, `bincode`, `lru`), declare all 22 features from plan.md §Project Structure §Structure Decision (4) — each feature gates only the crates it needs
- [ ] T006 Create `eval-judges/src/lib.rs` with `#![forbid(unsafe_code)]`, module declarations (`client`, and one `cfg`-gated module per provider), public re-exports of each `<Provider>JudgeClient` and its `Blocking<Provider>JudgeClient` wrapper behind their feature flags
- [ ] T007 Create `eval-judges/src/client.rs` stub with `BlockingExt` helper + `build_retry(policy: &RetryPolicy) -> ExponentialBuilder` used by every provider impl
- [ ] T008 Create stub files (empty `pub struct` + empty `impl JudgeClient`) for each provider in `eval-judges/src/`: `anthropic.rs`, `openai.rs`, `bedrock.rs`, `gemini.rs`, `mistral.rs`, `azure.rs`, `xai.rs`, `ollama.rs`, `proxy.rs` — one per feature, each with `#[cfg(feature = "<name>")]`
- [ ] T009 Extend `eval/src/lib.rs` module declarations: add `prompt`, `judge`, `aggregator`, `evaluators`, `simulation`, `generation`, `trace`, `telemetry`, `cache`, `report` modules, each gated appropriately
- [ ] T010 Verify `cargo build --workspace --no-default-features` succeeds (default build, no new surfaces active; FR-047 baseline validated)
- [ ] T011 Verify `cargo build -p swink-agent-eval --features all-evaluators,simulation,generation,trace-ingest,telemetry,html-report,langsmith,cli --no-default-features` compiles (all-features dry-run)

**Checkpoint**: Both crates compile with and without features. Nothing implemented yet — only scaffolding.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Shared types and traits every user story depends on. No user story may begin until this phase is complete.

### Shared types

- [ ] T012 Implement `Attachment` enum + `MaterializedAttachment` struct + `AttachmentError` enum in `eval/src/types.rs` per data-model.md §10
- [ ] T013 [P] Implement `UrlFilter` trait + `DefaultUrlFilter` (RFC 1918, loopback, link-local, metadata endpoint denials; HTTPS-required by default) in `eval/src/url_filter.rs` — always-on top-level module (not under `judge/`) so attachment materialization works without `judge-core`; re-export from `swink_agent_eval::judge` for ergonomic access when `judge-core` is enabled, per research.md §R-011
- [ ] T014 [P] Implement `Assertion` + `AssertionKind` + `InteractionExpectation` + `FewShotExample` structs in `eval/src/types.rs` per data-model.md §10
- [ ] T015 Extend `EvalCase` in `eval/src/types.rs` with new optional fields (`expected_assertion`, `expected_interactions`, `few_shot_examples`, `attachments`, `session_id`, `metadata`), preserving serde backwards-compatibility
- [ ] T016 Implement `CaseFingerprint` struct + `CASE_NAMESPACE: Uuid` constant (pinned project-specific namespace, derived once from `Uuid::new_v5(&Uuid::NAMESPACE_OID, b"swink-agent-eval.case")` and hard-coded as 16 bytes) + `default_session_id()` method on `EvalCase` producing deterministic UUID v5 from SHA-256 of canonical bincode bytes, per research.md §R-014; include a unit test verifying `CASE_NAMESPACE == Uuid::new_v5(&Uuid::NAMESPACE_OID, b"swink-agent-eval.case")`
- [ ] T017 Implement `Attachment::materialize(&Path, &dyn UrlFilter) -> Result<MaterializedAttachment, AttachmentError>` handling all three variants (Path read, Base64 decode, Url fetch with filter + HTTPS requirement) in `eval/src/types.rs`
- [ ] T018 [P] Write tests in `eval/tests/attachment_test.rs` covering: `Path` resolution relative to eval-set root, `Base64` round-trip, `Url` blocked by default filter (10.0.0.1, 127.0.0.1, 169.254.169.254), `Url` allowed with custom filter, unsupported MIME, missing path

### Aggregators

- [ ] T019 [P] Implement `Aggregator` trait in `eval/src/aggregator/mod.rs`
- [ ] T020 [P] Implement `Average`, `AllPass`, `AnyPass`, `Weighted` aggregators in `eval/src/aggregator/mod.rs`
- [ ] T021 [P] Write tests in `eval/tests/aggregator_test.rs` for each aggregator on happy path, empty input, single-sample, partial failures, and weighted-mismatch error

### Prompt templating

- [ ] T022 Implement `JudgePromptTemplate` trait + `PromptFamily` enum + `PromptContext` struct + `PromptError` enum in `eval/src/prompt/mod.rs` per data-model.md §1 (feature `judge-core`)
- [ ] T023 Implement `PromptTemplateRegistry` with `builtin()`, `get()`, `register()` methods rejecting duplicate versions in `eval/src/prompt/mod.rs`
- [ ] T024 Implement `MinijinjaTemplate` struct wrapping `minijinja::Environment` that validates named variables at construction time and returns `PromptError::MissingVariable` when a case lacks a referenced variable, in `eval/src/prompt/minijinja_template.rs`
- [ ] T025 [P] Write tests in `eval/tests/prompt_template_test.rs`: missing variable → deterministic error, valid render, version identifier preserved, duplicate version registration rejected, few-shot example injection order

### Judge infrastructure

- [ ] T026 Implement `RetryPolicy` struct + `Default` impl (6 attempts / 4 min / jitter true) in `eval/src/judge/mod.rs`
- [ ] T027 Implement `JudgeCache` + `CacheKey` with in-memory LRU (capacity 1024 default), SHA-256 prompt+model key, put/get/evict in `eval/src/judge/cache.rs`
- [ ] T028 Implement disk-backed `JudgeCache` variant (`with_disk(path)` constructor; JSON files per key; warm-load on construction; flush on `Drop`) in `eval/src/judge/cache.rs`
- [ ] T029 Implement `JudgeRegistry` + `JudgeRegistryBuilder` + `JudgeRegistryError` in `eval/src/judge/mod.rs` — constructor requires explicit `model_id` (Q9 clarification); validates `batch_size ∈ [1,128]` and `max_attempts ≤ 16`
- [ ] T030 [P] Write tests in `eval/tests/judge_registry_test.rs`: `model_id` empty rejected, `batch_size=0` rejected, `batch_size=129` rejected, default retry policy values, cache get/put/evict on LRU bound

### EvalCase extensions

- [ ] T031 Extend `EvalCase::validate()` in `eval/src/types.rs` to check new fields (`case_id` uniqueness within set, attachment format, assertion kind legality)
- [ ] T032 [P] Write tests in `eval/tests/eval_case_test.rs` for the extended validators and deterministic-session-id property (same case bytes → same UUID across re-runs)

### Panic isolation helpers

- [ ] T033 Implement `isolate_panic` wrapper in `eval/src/evaluator.rs` that wraps any async evaluator/simulator/generator/reporter call in `tokio::spawn(..).await` and converts `JoinError::is_panic()` to `Score::fail()` with `PanicDetail { location, message }`, per research.md §R-021
- [ ] T034 Extend `EvaluatorRegistry` in `eval/src/evaluator.rs` to route every evaluator call through `isolate_panic`; ensure existing 023/024 tests still pass

### Test scaffolding

- [ ] T035 Create `eval/tests/common/judge_fixtures.rs` with `wiremock`-based provider fixtures reusable across all judge client and evaluator tests
- [ ] T036 Create `eval-judges/tests/common/mod.rs` with shared `wiremock` fixtures for each provider's expected request/response shape

**Checkpoint**: Foundation complete. Shared types, prompt registry, judge infrastructure, cache, panic isolation, and test fixtures ready. User-story phases can begin.

---

## Phase 3: User Story 1 — Score Agent Runs with Production LLM Judges (P1) 🎯 MVP

**Goal**: Register an evaluator registry with real provider judges, run an eval set, get structured per-evaluator scores with reasons.

**Independent Test**: Run an eval set with at least one case per evaluator family against a live provider (or a `wiremock` double); every applicable evaluator returns a score with non-empty reason, non-applicable ones return no entry, single rate-limit blip is absorbed by retry.

### Judge clients in `eval-judges`

- [ ] T037 [P] [US1] Write `eval-judges/tests/anthropic_test.rs` covering: `wiremock`-backed happy path, rate-limit 429 absorbed by retry, exhausted retries surface `JudgeError::RateLimitExhausted`, malformed response → `JudgeError::MalformedResponse`, cancellation propagation
- [ ] T038 [P] [US1] Implement `AnthropicJudgeClient` + `BlockingAnthropicJudgeClient` in `eval-judges/src/anthropic.rs` — wraps `AnthropicAdapter`, exposes retry policy, batch size, async + blocking interfaces
- [ ] T039 [P] [US1] Implement `OpenAIJudgeClient` + blocking wrapper in `eval-judges/src/openai.rs` with matching test file `eval-judges/tests/openai_test.rs`
- [ ] T040 [P] [US1] Implement `BedrockJudgeClient` + blocking wrapper in `eval-judges/src/bedrock.rs` with test file `eval-judges/tests/bedrock_test.rs`
- [ ] T041 [P] [US1] Implement `GeminiJudgeClient` + blocking wrapper in `eval-judges/src/gemini.rs` with test file `eval-judges/tests/gemini_test.rs`
- [ ] T042 [P] [US1] Implement `MistralJudgeClient` + blocking wrapper in `eval-judges/src/mistral.rs` with test file `eval-judges/tests/mistral_test.rs`
- [ ] T043 [P] [US1] Implement `AzureJudgeClient` + blocking wrapper in `eval-judges/src/azure.rs` with test file `eval-judges/tests/azure_test.rs`
- [ ] T044 [P] [US1] Implement `XaiJudgeClient` + blocking wrapper in `eval-judges/src/xai.rs` with test file `eval-judges/tests/xai_test.rs`
- [ ] T045 [P] [US1] Implement `OllamaJudgeClient` + blocking wrapper in `eval-judges/src/ollama.rs` with test file `eval-judges/tests/ollama_test.rs`
- [ ] T046 [P] [US1] Implement `ProxyJudgeClient` + blocking wrapper in `eval-judges/src/proxy.rs` with test file `eval-judges/tests/proxy_test.rs`
- [ ] T047 [US1] Implement shared `backon`-based retry + cancellation wiring in `eval-judges/src/client.rs` (6 attempts / 4 min max / jitter / `CancellationToken`-aware) used by every provider
- [ ] T048 [US1] Implement request-batching wrapper in `eval-judges/src/client.rs` with configurable batch size ∈ [1, 128]; providers that don't support native batching fall through to sequential dispatch

### Prompt templates (built-in _v0, feature `judge-core`)

- [ ] T049 [P] [US1] Author quality-family prompt templates (`helpfulness_v0`, `correctness_v0`, `conciseness_v0`, `coherence_v0`, `response_relevance_v0`, `hallucination_v0`, `faithfulness_v0`, `plan_adherence_v0`, `laziness_v0`, `goal_success_rate_v0`) in `eval/src/prompt/templates/quality.rs` with minijinja source strings; faithfulness and hallucination have distinct, non-overlapping rubrics per Q1 clarification
- [ ] T050 [P] [US1] Author safety-family prompt templates (`harmfulness_v0`, `toxicity_v0`, `fairness_v0`, `pii_leakage_v0`, `prompt_injection_v0`, `code_injection_v0`) in `eval/src/prompt/templates/safety.rs`; harmfulness and toxicity have distinct rubrics per Q1 clarification (toxicity narrower: hate/harassment/slurs)
- [ ] T051 [P] [US1] Author RAG-family prompt templates (`rag_groundedness_v0`, `rag_retrieval_relevance_v0`, `rag_helpfulness_v0`) in `eval/src/prompt/templates/rag.rs`
- [ ] T052 [P] [US1] Author agent-family prompt templates (`trajectory_accuracy_v0`, `trajectory_accuracy_with_ref_v0`, `task_completion_v0`, `user_satisfaction_v0`, `agent_tone_v0`, `knowledge_retention_v0`, `language_detection_v0`, `perceived_error_v0`, `interactions_v0`) in `eval/src/prompt/templates/agent.rs`
- [ ] T053 [P] [US1] Author code + multimodal prompt templates (`code_llm_judge_v0`, `image_safety_v0`) in `eval/src/prompt/templates/code.rs` and `eval/src/prompt/templates/multimodal.rs`
- [ ] T054 [US1] Register all built-in templates in `PromptTemplateRegistry::builtin()` in `eval/src/prompt/mod.rs`; add a test asserting every expected version identifier is present

### Shared evaluator config

- [ ] T055 [US1] Implement `JudgeEvaluatorConfig` + `Default::default_with(judge_registry)` in `eval/src/evaluators/mod.rs`
- [ ] T056 [US1] Implement shared `dispatch_judge()` helper in `eval/src/evaluators/mod.rs` that renders the config's (or builtin) prompt, dispatches via `JudgeRegistry`, records `prompt_version` in the resulting `EvalMetricResult::details`, clamps the returned score to `[0.0, 1.0]` and surfaces any out-of-range value as `details.push(Detail::ScoreClamped { original, clamped })` (per FR-021 extended requirement), and returns `None` when the case doesn't populate the evaluator's criterion fields (per FR-020)

### Quality family (feature `evaluator-quality`)

- [ ] T057 [P] [US1] Write tests for `HelpfulnessEvaluator`, `CorrectnessEvaluator`, `ConcisenessEvaluator`, `CoherenceEvaluator` in `eval/tests/evaluators_quality_test.rs` using `MockJudge`
- [ ] T058 [P] [US1] Implement `HelpfulnessEvaluator`, `CorrectnessEvaluator`, `ConcisenessEvaluator`, `CoherenceEvaluator`, `ResponseRelevanceEvaluator` in `eval/src/evaluators/quality.rs` — each `None`-returns when criterion absent, records `prompt_version`, uses `Average` aggregator
- [ ] T059 [P] [US1] Implement `HallucinationEvaluator`, `FaithfulnessEvaluator`, `PlanAdherenceEvaluator`, `LazinessEvaluator`, `GoalSuccessRateEvaluator` in `eval/src/evaluators/quality.rs`; `GoalSuccessRateEvaluator` consumes `expected_assertion`
- [ ] T060 [P] [US1] Write tests in `eval/tests/evaluators_quality_test.rs` covering: hallucination distinct from faithfulness (retrieved-context vs. model-knowledge rubric separation), `None`-return when case doesn't populate criterion, score clamp to [0.0, 1.0] with warning

### Safety family (feature `evaluator-safety`, default aggregator `AllPass`)

- [ ] T061 [P] [US1] Write tests in `eval/tests/evaluators_safety_test.rs` covering: PII detection with at least three entity classes, prompt-injection detection, harmfulness distinct from toxicity (broader vs. narrower rubric)
- [ ] T062 [P] [US1] Implement `HarmfulnessEvaluator`, `ToxicityEvaluator`, `FairnessEvaluator` in `eval/src/evaluators/safety.rs` — binary scores; default aggregator `AllPass` explicitly set in constructor
- [ ] T063 [P] [US1] Implement `PIILeakageEvaluator` + `PIIClass` enum (Email/Phone/SSN/CreditCard/IpAddress/ApiKey/PersonalName/Address/Other) in `eval/src/evaluators/safety.rs`; constructor accepts `entity_classes: Vec<PIIClass>` (default: all built-in)
- [ ] T064 [P] [US1] Implement `PromptInjectionEvaluator` and `CodeInjectionEvaluator` in `eval/src/evaluators/safety.rs`

### RAG family (feature `evaluator-rag`)

- [ ] T065 [P] [US1] Write tests in `eval/tests/evaluators_rag_test.rs` covering: groundedness-against-retrieved-context, retrieval-relevance, embedding-similarity with a `StubEmbedder` test double
- [ ] T066 [P] [US1] Implement `RAGGroundednessEvaluator`, `RAGRetrievalRelevanceEvaluator`, `RAGHelpfulnessEvaluator` in `eval/src/evaluators/rag.rs`
- [ ] T067 [P] [US1] Implement `Embedder` trait + `EmbedderError` enum + `EmbeddingSimilarityEvaluator` (deterministic cosine similarity with configurable threshold, default 0.8) in `eval/src/evaluators/rag.rs`

### Agent family (feature `evaluator-agent`)

- [ ] T068 [P] [US1] Write tests in `eval/tests/evaluators_agent_test.rs` covering: trajectory accuracy with and without reference, task-completion consumes `expected_assertion`, interactions consumes `expected_interactions`, language-detection returns detected code in details
- [ ] T069 [P] [US1] Implement `TrajectoryAccuracyEvaluator`, `TrajectoryAccuracyWithRefEvaluator`, `TaskCompletionEvaluator`, `UserSatisfactionEvaluator` in `eval/src/evaluators/agent.rs`
- [ ] T070 [P] [US1] Implement `AgentToneEvaluator`, `KnowledgeRetentionEvaluator`, `LanguageDetectionEvaluator`, `PerceivedErrorEvaluator`, `InteractionsEvaluator` in `eval/src/evaluators/agent.rs`

### Structured family (feature `evaluator-structured`)

- [ ] T071 [P] [US1] Write tests in `eval/tests/evaluators_structured_test.rs` covering: per-key rubric application, `exclude_keys` filter, malformed JSON → `JudgeError::MalformedResponse`, schema validation happy + unhappy path
- [ ] T072 [P] [US1] Implement `JsonMatchEvaluator` + `KeyStrategy` enum (Average/All/None/Rubric) in `eval/src/evaluators/structured.rs`
- [ ] T073 [P] [US1] Implement `JsonSchemaEvaluator` (deterministic; compiles schema via `jsonschema` crate; no judge call) in `eval/src/evaluators/structured.rs`

### Simple family (feature `evaluator-simple`)

- [ ] T074 [P] [US1] Write tests in `eval/tests/evaluators_simple_test.rs` covering: exact-match case-sensitivity & trim toggle, Levenshtein normalized similarity threshold
- [ ] T075 [P] [US1] Implement `ExactMatchEvaluator` + `LevenshteinDistanceEvaluator` in `eval/src/evaluators/simple.rs`

### Code family (feature `evaluator-code`; sandbox behind `evaluator-sandbox`)

- [ ] T076 [P] [US1] Write tests in `eval/tests/evaluators_code_test.rs` covering: cargo-check happy path, clippy warning surface, `CodeExtractor` strategies (markdown-fence, regex, LLM)
- [ ] T077 [P] [US1] Implement `CargoCheckEvaluator` + `ClippyEvaluator` in `eval/src/evaluators/code/cargo_check.rs` and `eval/src/evaluators/code/clippy.rs` — deterministic, shell out to cargo in a tempdir
- [ ] T078 [P] [US1] Implement `CodeExtractor` + `CodeExtractorStrategy` enum in `eval/src/evaluators/code/extractor.rs`
- [ ] T079 [P] [US1] Implement `CodeLlmJudgeEvaluator` in `eval/src/evaluators/code/llm_judge.rs` using the `code_llm_judge_v0` template
- [ ] T080 [US1] Implement `SandboxLimits` struct + `Default` impl (120 s wall / 60 s CPU / 1 GiB RSS / 256 FDs / no network) in `eval/src/evaluators/code/sandbox.rs`
- [ ] T081 [US1] Implement `SandboxedExecutionEvaluator` Unix path: `cfg(target_family = "unix")` module with `#![allow(unsafe_code)]` exception + safe `posix` submodule using `libc::setrlimit`/`libc::unshare`/`libc::prlimit` wrapped with `// SAFETY:` invariants; spawns child in tempdir; enforces all five limits; produces `EvaluatorError::SandboxLimitExceeded { limit }` when a bound is hit — per research.md §R-006
- [ ] T082 [US1] Implement `SandboxedExecutionEvaluator` Windows stub: `cfg(target_family = "windows")` produces `EvaluatorError::UnsupportedPlatform` at evaluation time, never panics
- [ ] T083 [P] [US1] Write tests in `eval/tests/evaluators_sandbox_test.rs` (Unix only via `cfg`): each limit enforced, wall-clock timeout cancels child, FD bomb caught, memory bomb caught, network egress blocked

### Multimodal family (feature `multimodal`)

- [ ] T084 [P] [US1] Write tests in `eval/tests/evaluators_multimodal_test.rs` covering: image-safety evaluator happy + deny path, attachment materialization integration
- [ ] T085 [P] [US1] Implement `ImageSafetyEvaluator` in `eval/src/evaluators/multimodal.rs` consuming `Attachment::Path`/`Base64`/`Url` via materialization pipeline
- [ ] T086 [US1] Wire attachment materialization into the shared `dispatch_judge` helper so any evaluator can reference attachments

### Registry wiring

- [ ] T087 [US1] Update `EvaluatorRegistry::add` to validate evaluator-name uniqueness within a registry (spec edge case) and surface `EvalError::DuplicateEvaluator` on collision
- [ ] T088 [US1] Update `eval/src/lib.rs` to re-export every US1 surface behind the correct feature gates
- [ ] T089 [US1] Integration test in `eval/tests/us1_end_to_end_test.rs`: registry with one evaluator per family, `wiremock`-backed judge returning canned verdicts, assert per-evaluator score + reason + prompt_version recorded, non-applicable evaluators return no entry

**Checkpoint**: MVP step 1 complete — real judges + 24 evaluators + all 9 provider clients working end-to-end.

---

## Phase 4: User Story 2 — Run Large Eval Sets Fast and Repeatably (P1) 🎯 MVP

**Goal**: Parallelism + `num_runs` + task-result caching + cancellation + `initial_session_file` all working on `EvalRunner`.

**Independent Test**: 20-case suite run twice; second run reuses cached invocations, completes in a fraction of first run's time, agent is not invoked; cancellation mid-run returns partial results.

### Runner extensions

- [x] T090 [P] [US2] Write tests in `eval/tests/runner_parallelism_test.rs`: `with_parallelism(4)` causes up to 4 concurrent case executions; `parallelism=1` behaves exactly as sequential baseline (per FR-036); permits release after case completion
- [x] T091 [US2] Extend `EvalRunner` with `parallelism: usize` field and `with_parallelism(n)` builder (`panic` on `n == 0`); wire `tokio::sync::Semaphore` permit acquisition around per-case execution in `eval/src/runner.rs`
- [x] T092 [P] [US2] Write tests in `eval/tests/runner_num_runs_test.rs`: `num_runs=3` yields three per-evaluator samples per case, variance reported, cached invocation shared across all N runs (Q2 clarification)
- [x] T093 [US2] Extend `EvalRunner` with `num_runs: u32` field and `with_num_runs(n)` builder (`panic` on `n == 0`); loop judge dispatch N times per case using the same `Invocation`; compute `std_dev` into `RunnerMetricSample` in `eval/src/runner.rs`
- [x] T094 [P] [US2] Write tests in `eval/tests/cache_test.rs`: cache hit reuses `Invocation`, cache miss re-invokes agent, case-input change invalidates key, disk cache round-trip, single cached invocation served to all `num_runs` iterations
- [x] T095 [US2] Implement `EvaluationDataStore` trait + `StoreError` in `eval/src/cache.rs`
- [x] T096 [US2] Implement `LocalFileTaskResultStore` with disk layout `<root>/<eval_set_id>/<case_id>/<fingerprint_hex>.json` in `eval/src/cache.rs`
- [x] T097 [US2] Implement `CaseFingerprint` canonicalization (case_id, system_prompt, user_messages, initial_session, tool_set_hash, agent_model) and SHA-256 of bincode bytes producing `CacheKey` in `eval/src/cache.rs`; wire into `EvalRunner::run_set`
- [x] T098 [US2] Extend `EvalRunner` with `cache: Option<Arc<dyn EvaluationDataStore>>` + `with_cache(store)` builder in `eval/src/runner.rs`

### Cancellation

- [x] T099 [P] [US2] Write tests in `eval/tests/runner_cancel_test.rs`: cancellation mid-run returns partial result with cancellation indicator; in-flight agent and judge calls honor the token; completed cases keep their results
- [x] T100 [US2] Extend `EvalRunner` with `cancel: Option<CancellationToken>` + `with_cancellation(tok)` builder; propagate the token into agent calls and judge calls via `tokio::select!` at every await point in `eval/src/runner.rs`

### Initial session

- [x] T101 [P] [US2] Write tests in `eval/tests/runner_initial_session_test.rs`: `initial_session_file` loaded; baseline session context present on each case start; missing file surfaces a clear error, not a panic
- [x] T102 [US2] Extend `EvalRunner` with `initial_session_file: Option<PathBuf>` + `with_initial_session_file(path)` builder; parse JSON per research.md §R-023 matching spec-034 `SessionState` serde shape in `eval/src/runner.rs`

### US2 integration

- [x] T103 [US2] Integration test in `eval/tests/us2_end_to_end_test.rs`: 20-case suite / parallelism 4 / num_runs 3 / cache hit → second-run wall-clock is ≤ 20 % of first-run wall-clock with agent invocation count = 0, per SC-002/SC-003

**Checkpoint**: MVP step 2 complete — runner fast, repeatable, resumable.

---

## Phase 5: User Story 3 — Configure and Version Prompt Templates (P1) 🎯 MVP

**Goal**: Every evaluator accepts custom prompt, few-shot examples, system prompt, output schema, use-reasoning flag — without code change.

**Independent Test**: Override built-in `CorrectnessEvaluator`'s prompt with a custom template and few-shot example; verify rendered prompt reflects overrides and result records the custom prompt version.

- [ ] T104 [P] [US3] Write tests in `eval/tests/us3_custom_prompt_test.rs`: `.with_prompt(custom_template)` replaces built-in at evaluation time; few-shot examples inject at declared positions; bumped `_v0` → `_v1` is explicit opt-in (old version remains accessible); variable missing at construction → deterministic error
- [ ] T105 [US3] Extend every judge-backed evaluator's builder with `.with_prompt(Arc<dyn JudgePromptTemplate>)`, `.with_few_shot(Vec<FewShotExample>)`, `.with_system_prompt(String)`, `.with_output_schema(JsonSchema)`, `.with_use_reasoning(bool)`, `.with_feedback_key(String)` — all route through the shared `JudgeEvaluatorConfig`
- [ ] T106 [US3] Ensure `prompt_version` is recorded in every `EvalMetricResult::details` for every judge-backed evaluator (reused from T056 but re-verified here)
- [ ] T107 [US3] Add a versioning smoke test: registry contains `correctness_v0` (built-in) + `correctness_v1` (custom); two cases use each; results distinguish them per-metric

**Checkpoint**: MVP step 3 complete — prompts are configuration; version discipline enforced.

**MVP complete after Phase 5. Remaining phases (US4–US9) are additive and independently mergeable.**

---

## Phase 6: User Story 4 — Multi-Turn Dialogues via Simulated Users and Tools (P2)

**Goal**: `ActorSimulator` + `ToolSimulator` + `run_multiturn_simulation` produce an `Invocation` scorable by any evaluator.

**Independent Test**: 5-turn scripted scenario, `ToolSimulator` providing tool responses, resulting `Invocation` contains 3 user turns + correct tool-call/tool-result pairings, then scored by `GoalSuccessRateEvaluator`.

*Depends on US1 (needs `JudgeClient`).*

- [ ] T108 [P] [US4] Write tests in `eval/tests/simulation_test.rs`: 5-turn dialogue, goal-completion signal fires at turn 3, `max_turns` reached without goal triggers graceful termination
- [ ] T109 [US4] Implement `ActorProfile` struct + `ActorSimulator` (profile, judge, greeting pool, turn cap, goal-completion signal) in `eval/src/simulation/actor.rs`
- [ ] T110 [US4] Implement `StateRegistry` + `StateBucket` with bounded FIFO history (configurable `history_cap`, default 32) in `eval/src/simulation/tool.rs`
- [ ] T111 [US4] Implement `ToolSimulator` that generates schema-valid responses (via `jsonschema` validation) and records calls in its bucket in `eval/src/simulation/tool.rs`; schema-invalid response surfaces `SimulationError::SchemaValidation`, never silent
- [ ] T112 [US4] Implement `run_multiturn_simulation(agent, actor, tool_sim, max_turns, cancel)` orchestrator in `eval/src/simulation/orchestrator.rs`; produces full `Invocation`; honors cancellation cooperatively
- [ ] T113 [P] [US4] Write tests in `eval/tests/simulation_state_test.rs`: shared-state semantics within a bucket across two tools, FIFO eviction at history cap, bucket separation across `state_key`
- [ ] T114 [US4] Integration test in `eval/tests/us4_end_to_end_test.rs`: simulated conversation scored by `GoalSuccessRateEvaluator` produces identical metrics to an equivalent real `Invocation` (transparency to scoring layer, per US4 scenario 5)

**Checkpoint**: Simulation fully integrated; scorable end-to-end.

---

## Phase 7: User Story 5 — Auto-Generate Diverse Test Cases from a Context Description (P2)

**Goal**: `ExperimentGenerator` + `TopicPlanner` produce validated `EvalSet` from a description.

**Independent Test**: `desired_count=12`, `num_topics=4` yields 12 valid `EvalCase`s spanning 4 topics; retries absorb malformed judge output.

*Depends on US1.*

- [ ] T115 [P] [US5] Write tests in `eval/tests/generation_test.rs`: 20 cases / 5 topics distribution, tools-scoped trajectories reference only provided tools, every emitted case passes `EvalCase::validate()`, retries on malformed responses before skipping a slot
- [ ] T116 [US5] Implement `TopicPlanner` in `eval/src/generation/topic.rs` producing `Vec<TopicSlot>` with even distribution
- [ ] T117 [US5] Implement `ExperimentGenerator` + `GenerationRequest` struct in `eval/src/generation/experiment.rs`; retries up to bounded cap on malformed JSON; every emitted case validated before returning
- [ ] T118 [US5] Integration test in `eval/tests/us5_end_to_end_test.rs`: generated `EvalSet` loaded into `EvalRunner` executes successfully; tools-scoped trajectories respected when `agent_tools` provided

**Checkpoint**: Auto-generation integrated; generator ↔ runner round-trip validated.

---

## Phase 8: User Story 6 — Ingest Agent Traces from External Observability Backends (P2)

**Goal**: `TraceProvider` + `SessionMapper` + `TraceExtractor` pull sessions from Langfuse/OpenSearch/CloudWatch/OTLP into `Invocation`.

**Independent Test**: Recorded in-process run via in-memory OTel exporter re-loaded and scored — bitwise-identical scores to original run for deterministic evaluators.

*Independent of US1 for core trait; dependent for scoring tests.*

### Core trace surface (feature `trace-ingest`)

- [ ] T119 [P] [US6] Write tests in `eval/tests/trace_ingest_test.rs`: `OtelInMemoryTraceProvider` round-trip (record + re-load), missing attribute → `MappingError::MissingAttribute`, partially-written session → `TraceProviderError::SessionInProgress`
- [ ] T120 [US6] Implement `TraceProvider` trait + `TraceProviderError` + `RawSession` in `eval/src/trace/provider.rs`
- [ ] T121 [US6] Implement `OtelInMemoryTraceProvider` wrapping `opentelemetry-sdk`'s `InMemorySpanExporter` in `eval/src/trace/provider.rs`
- [ ] T122 [US6] Implement `SessionMapper` trait + `MappingError` in `eval/src/trace/mapper.rs`
- [ ] T123 [P] [US6] Implement `OpenInferenceSessionMapper` in `eval/src/trace/mapper.rs`
- [ ] T124 [P] [US6] Implement `LangChainSessionMapper` in `eval/src/trace/mapper.rs`
- [ ] T125 [US6] Implement `OtelGenAiSessionMapper` + `GenAIConventionVersion` enum (V1_27, V1_30, Experimental) with per-version attribute-mapping tables in `eval/src/trace/mapper.rs`

### Per-backend providers

- [ ] T126 [P] [US6] Implement `OtlpHttpTraceProvider` (feature `trace-otlp`) in `eval/src/trace/otlp.rs` with `reqwest`-based OTLP-HTTP pull and test in `eval/tests/trace_otlp_test.rs` (wiremock-backed)
- [ ] T127 [P] [US6] Implement `LangfuseTraceProvider` (feature `trace-langfuse`) in `eval/src/trace/langfuse.rs` with test in `eval/tests/trace_langfuse_test.rs`
- [ ] T128 [P] [US6] Implement `OpenSearchTraceProvider` (feature `trace-opensearch`) in `eval/src/trace/opensearch.rs` with test
- [ ] T129 [P] [US6] Implement `CloudWatchTraceProvider` (feature `trace-cloudwatch`) in `eval/src/trace/cloudwatch.rs` with test
- [ ] T130 [US6] Feature-gate error: disabled-feature access to a backend provider produces a clear compile-time or construction-time error (spec US6 scenario 4); add a doc-test covering this

### Extractors

- [ ] T131 [US6] Implement `EvaluationLevel` enum + `TraceExtractor` trait in `eval/src/trace/extractor.rs`
- [ ] T132 [P] [US6] Implement `SwarmExtractor` consuming spec-040 swarm result types in `eval/src/trace/extractor.rs`
- [ ] T133 [P] [US6] Implement `GraphExtractor` consuming spec-039 graph result types in `eval/src/trace/extractor.rs`
- [ ] T134 [US6] Integration test in `eval/tests/us6_end_to_end_test.rs`: record in-process run via in-memory exporter; re-load via `OtelInMemoryTraceProvider` + `OpenInferenceSessionMapper`; score with deterministic evaluators; assert bit-identical scores (per SC-008)

**Checkpoint**: Trace ingestion working across all four backends plus in-memory.

---

## Phase 9: User Story 7 — Emit OTel Spans for Eval Runs Themselves (P2)

**Goal**: `EvalsTelemetry` emits the three-level span tree per FR-035.

**Independent Test**: Run eval set with `EvalsTelemetry` pointing at the in-memory exporter; assert expected span tree and attribute set.

*Depends on US2 (extends `EvalRunner`).*

- [ ] T135 [P] [US7] Write tests in `eval/tests/telemetry_test.rs` using `opentelemetry-sdk::testing::trace::InMemorySpanExporter`: root `swink.eval.run_set`, per-case `swink.eval.case`, per-evaluator `swink.eval.evaluator`; failed case records OTel status-error + exception event; parent span is inherited when one exists
- [ ] T136 [US7] Implement `EvalsTelemetry` + `EvalsTelemetryBuilder` in `eval/src/telemetry.rs` (feature `telemetry`)
- [ ] T137 [US7] Wire `EvalsTelemetry` into `EvalRunner::run_set` in `eval/src/runner.rs`: emit the three-level span tree; attach standardized attributes per FR-035; honor existing parent span when one is active
- [ ] T138 [US7] Integration test in `eval/tests/us7_end_to_end_test.rs`: full run produces the expected span tree; regression in `correctness` at a known case surfaces as an errored span (per US7 scenario 3)

**Checkpoint**: Eval runs emit OTel spans.

---

## Phase 10: User Story 8 — Produce CI- and Human-Friendly Reports (P2)

**Goal**: `ConsoleReporter` (plain-text), `JsonReporter`, `MarkdownReporter` always-on; `HtmlReporter` behind `html-report`; `LangSmithExporter` behind `langsmith`.

**Independent Test**: Same `EvalSetResult` through each reporter; each output parses / renders cleanly; each contains the expected per-case / per-metric detail.

*Independent of other stories.*

### Reporters

- [ ] T139 [P] [US8] Write tests in `eval/tests/reporter_console_test.rs`: plain-text line-oriented output, one line per case verdict + indented evaluator score+reason, no ANSI, no cursor control, no interactivity (per Q8 clarification)
- [ ] T140 [US8] Implement `Reporter` trait + `ReporterOutput` enum + `ReporterError` in `eval/src/report/mod.rs`
- [ ] T141 [P] [US8] Implement `ConsoleReporter` in `eval/src/report/console.rs` (always-on, plain-text)
- [ ] T142 [P] [US8] Write tests in `eval/tests/reporter_json_test.rs`: self-contained JSON; schema validation against `eval-result.schema.json`
- [ ] T143 [P] [US8] Implement `JsonReporter` in `eval/src/report/json.rs` (always-on) + author `specs/043-evals-adv-features/contracts/eval-result.schema.json`
- [ ] T144 [P] [US8] Write tests in `eval/tests/reporter_markdown_test.rs`: valid Markdown table; no ANSI; per-case and per-metric detail present
- [ ] T145 [P] [US8] Implement `MarkdownReporter` in `eval/src/report/markdown.rs` (always-on, PR-comment-ready)
- [ ] T146 [P] [US8] Write tests in `eval/tests/reporter_html_test.rs`: single self-contained file; `<details>`/`<summary>` collapsibility; no external asset dependencies; bounded output size for thousand-case results
- [ ] T147 [US8] Implement `HtmlReporter` in `eval/src/report/html.rs` using `askama` templates with inlined CSS/JS (feature `html-report`); embed template at compile time
- [ ] T148 [P] [US8] Write tests in `eval/tests/reporter_langsmith_test.rs` (wiremock-backed): `EvalSetResult` pushed as run; per-evaluator feedback attached under configured `feedback_key`; partial push failure surfaces `LangSmithExportError::Push { pushed, failed, first_error }`
- [ ] T149 [US8] Implement `LangSmithExporter` + `LangSmithExportError` in `eval/src/report/langsmith.rs` (feature `langsmith`) with `reqwest`-based POST to `/runs` and `/feedback` per research.md §R-015

### Report integration

- [ ] T150 [US8] Integration test in `eval/tests/us8_end_to_end_test.rs`: same `EvalSetResult` through each reporter; HTML output validates as well-formed HTML5; JSON output validates against schema; Markdown output validates as CommonMark

**Checkpoint**: All reporters functional; JSON schema published.

---

## Phase 11: User Story 9 — CI Integration and Local CLI (P3)

**Goal**: `swink-eval` CLI with `run` / `report` / `gate` subcommands; GitHub Actions workflow templates shipped.

**Independent Test**: Invoke CLI against an `EvalSet`; verify identical output to in-process API; re-render a persisted result with `report`; evaluate a gate with `gate`.

*Depends on US2 + US8 (runner + reporters).*

### CLI binary

- [ ] T151 [P] [US9] Write tests in `eval/tests/cli_test.rs` (using `assert_cmd` or equivalent spawning the binary): `run --set` produces matching output to in-process API; `report` re-renders without re-execution; `gate` returns 0/non-zero; missing file returns exit 2
- [ ] T152 [US9] Scaffold `eval/src/bin/swink_eval.rs` with `clap` derive-parser: `run { --set, --out, --parallelism, --reporter }`, `report { --result, --format }`, `gate { --result, --gate-config }` (feature `cli`)
- [ ] T153 [US9] Implement `run` subcommand: load `EvalSet`, construct `EvalRunner`, execute, render chosen reporter to stdout, write optional JSON artifact, exit with code derived from `GateConfig`
- [ ] T154 [US9] Implement `report` subcommand: load persisted `EvalSetResult`, render through chosen reporter, write to stdout (no re-execution)
- [ ] T155 [US9] Implement `gate` subcommand: load persisted `EvalSetResult`, load `GateConfig`, evaluate, return 0/1/2/3 per contracts/public-api.md; no stdout output

### CI templates

- [ ] T156 [P] [US9] Author `eval/src/ci/templates/pr-eval.yml` per research.md §R-018
- [ ] T157 [P] [US9] Author `eval/src/ci/templates/nightly-eval.yml`
- [ ] T158 [P] [US9] Author `eval/src/ci/templates/release-eval.yml`
- [ ] T159 [P] [US9] Author `eval/src/ci/templates/pre-commit-hook.yml`
- [ ] T160 [US9] Add `include_str!`-based compile-time references for all four templates so they're validated by `cargo build`

### US9 integration

- [ ] T161 [US9] Integration test in `eval/tests/us9_end_to_end_test.rs`: install binary into a tempdir, invoke `run → report → gate` pipeline against a fixture `EvalSet`; verify stable exit codes and identical outputs to the in-process API

**Checkpoint**: CLI shipping, CI templates authored.

---

## Phase 12: Polish & Cross-Cutting Concerns

**Purpose**: Finish the spec's non-functional contract, lift success-criteria gates, update docs, bump versions, run the full-features matrix.

### Feature-matrix verification

- [ ] T162 Verify `cargo build -p swink-agent-eval --no-default-features` succeeds; confirm default build pulls no new transitive deps beyond 023 baseline (SC-009)
- [ ] T163 Verify `cargo build -p swink-agent-eval --features all-evaluators,simulation,generation,trace-ingest,trace-otlp,trace-langfuse,trace-opensearch,trace-cloudwatch,telemetry,html-report,langsmith,cli` succeeds
- [ ] T164 Verify `cargo build -p swink-agent-eval-judges --features all-judges` succeeds
- [ ] T165 Run `cargo test --workspace` and confirm zero live LLM calls (FR-050); add a repo-level test asserting no test sets `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / etc. without feature `live-judges`

### Panic-isolation and cancellation sweep

- [ ] T166 Review every evaluator, simulator, generator, reporter for panic-isolation coverage (FR-021); add missing `isolate_panic` wrappers; add a dedicated test in `eval/tests/score_clamp_test.rs` asserting that a judge returning `1.3` produces a clamped `1.0` with `ScoreClamped { original: 1.3, clamped: 1.0 }` in details (spec Edge Cases + FR-021 extension)
- [ ] T167 Review every await point in runner / simulation / generation for `CancellationToken` responsiveness (FR-040); no task should hang after cancellation

### Docs and metadata

- [ ] T168 Update `eval/README.md` with feature-flag matrix and usage recipes from quickstart.md
- [ ] T169 Create `eval-judges/README.md` documenting each provider feature and credentials
- [ ] T170 Update `docs/HLD.md` with new crate count (14 → 15), new architectural surface (eval-judges, prompt registry, simulation, generation, trace ingestion, telemetry, reporters, CLI)
- [ ] T171 Update root `README.md` feature matrix to include the eval advanced-features surface
- [ ] T172 Update `CHANGELOG.md` with 043 scope, breaking-change notes (EvalCase extended; no default judge model; FR-044 removed — legacy converter never shipped)

### Success-criteria validation

- [ ] T173 Add a benchmark test asserting SC-002: 20-case / parallelism-4 suite completes in under 2× single-case wall-clock against a `wiremock`-backed "fast provider"
- [ ] T174 Add a benchmark test asserting SC-003: prompt-only re-run reuses cached invocations, agent invocation count = 0
- [ ] T175 Add a property test asserting SC-004: swapping judge model requires exactly one constructor-arg change; every existing evaluator continues to work against the new model
- [ ] T176 Add a deterministic-replay test asserting SC-008: OTel-traced run re-loaded via `OtelInMemoryTraceProvider` scores bit-identical to in-process run for deterministic evaluators
- [ ] T177 Add a regression test asserting SC-009: `cargo tree -p swink-agent-eval --no-default-features` produces an identical transitive dep graph to the 023 baseline

### Release hygiene

- [ ] T178 Run `cargo clippy --workspace --all-targets --all-features -- -D warnings` and resolve any new lints
- [ ] T179 Run `cargo fmt --all --check`
- [ ] T180 Bump `workspace.package.version` to `0.9.0` in root `Cargo.toml` (minor bump — new features additive, no removed items)
- [ ] T181 Verify Dependabot / `cargo deny check` for the new deps (`minijinja`, `backon`, `strsim`, `jsonschema`, `opentelemetry`, `askama`, `clap`, `libc`, `opentelemetry-otlp`)

---

## Dependencies

```text
Phase 1 (Setup) ──► Phase 2 (Foundational) ──► Phase 3 (US1, P1) ──┐
                                          └──► Phase 4 (US2, P1) ──┤──► Phase 7 (US5, P2) ──┐
                                          └──► Phase 5 (US3, P1) ──┤──► Phase 6 (US4, P2) ──┤
                                                                   │                        │
                                                                   ├──► Phase 9 (US7, P2) ──┤
                                                                   │                        │
                                          ┌──► Phase 8 (US6, P2) ──┤                        │
                                          │                        │                        │
                                          └──► Phase 10 (US8, P2) ─┴──► Phase 11 (US9, P3) ─┴──► Phase 12 (Polish)
```

- **Setup (Phase 1)** must complete before any foundational work.
- **Foundational (Phase 2)** must complete before any story phase.
- **US1, US2, US3** are all P1 and together form the MVP. Within the MVP:
  - US1 can run largely in parallel across its sub-phases (judge clients, prompt templates, evaluator families are each parallelizable).
  - US2 is structurally independent of US1 at file level (touches `runner.rs` / `cache.rs` / `types.rs` fingerprint). US2 tasks can parallelize with US1 tasks after Phase 2.
  - US3 is largely declarative over US1's surface (adds `.with_*()` builder methods already stubbed in `JudgeEvaluatorConfig`); can start as soon as US1 evaluators exist.
- **US4 / US5** depend on US1 (need `JudgeClient`). They can run in parallel with each other.
- **US6** is independent of US1 at the trait level; integration tests depend on US1 evaluators.
- **US7** extends the runner from US2.
- **US8** is largely independent of other stories (consumes `EvalSetResult`).
- **US9** depends on US2 (runner) + US8 (reporters).

## Parallel Execution Examples

### Judge clients (US1)
All nine provider clients (T037–T046) are file-level independent — each modifies exactly one file in `eval-judges/src/` and one file in `eval-judges/tests/`. Launch them in parallel:

```text
T037 anthropic   T038 openai   T039 bedrock   T040 gemini   T041 mistral
T042 azure       T043 xai      T044 ollama    T045 proxy
```

### Prompt templates (US1)
Five family files (T049–T053), each one author pass:

```text
T049 quality/    T050 safety/    T051 rag/    T052 agent/    T053 code+multimodal/
```

### Evaluator tests (US1)
Every family's test file can be authored in parallel (T057, T060, T061, T065, T068, T071, T074, T076, T083, T084); wire them after the evaluator impls land in the same family's `.rs` file.

### Trace provider backends (US6)
T126–T129 (OTLP / Langfuse / OpenSearch / CloudWatch) — file-level independent.

### CI templates (US9)
T156–T159 — four independent YAML files.

## Implementation Strategy

1. **Sequentially** run Phase 1 and Phase 2 — no story work may start until foundational types and trait surfaces exist.
2. **MVP (Phases 3 → 4 → 5)** can start once Phase 2 lands:
   - Kick off Phase 3 task fan-out (judge clients + prompt templates + evaluator families each parallelizable).
   - Phase 4 runner work is file-independent of Phase 3 and can run concurrently.
   - Phase 5 depends on Phase 3 evaluator scaffolds existing but its own tasks are small.
3. **Ship the MVP** — at this point every P1 acceptance scenario passes; `swink-eval` has no CLI yet but the in-process API is complete.
4. **P2 layer (Phases 6–10)** — each story is independently deliverable and each can ship as its own PR:
   - Simulation (6), Generation (7) — can run sequentially or in parallel; both consume `JudgeClient` only.
   - Trace ingestion (8) — largest P2, can be split by backend if desired.
   - Telemetry (9) — small; extends runner surface from Phase 4.
   - Reporters (10) — largely independent; can ship one reporter at a time if pressure dictates.
5. **P3 layer (Phase 11)** — CLI + CI templates; ships once P2 reporters and P1 runner are stable.
6. **Polish (Phase 12)** — last; gates the feature matrix, updates docs, bumps version, verifies success criteria.

**Estimated total task count**: 181 (T001–T181).

**Task-count-per-story**:

| Phase | Story | Count |
|---|---|---|
| 1 | Setup | 11 |
| 2 | Foundational | 25 |
| 3 | US1 | 53 |
| 4 | US2 | 14 |
| 5 | US3 | 4 |
| 6 | US4 | 7 |
| 7 | US5 | 4 |
| 8 | US6 | 16 |
| 9 | US7 | 4 |
| 10 | US8 | 12 |
| 11 | US9 | 11 |
| 12 | Polish | 20 |
| **Total** |  | **181** |
