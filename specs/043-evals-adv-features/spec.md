# Feature Specification: Evals: Advanced Features

**Feature Branch**: `043-evals-adv-features`
**Created**: 2026-04-21
**Status**: Draft
**Input**: User description: "Build the Advanced Evals layer of `swink-agent-eval` — concrete LLM-as-judge infrastructure, runner upgrades, simulation, generation, observability ingestion, and reporting. Consumes the trait surface introduced by 023-eval-trajectory-matching (`JudgeClient`, `JudgeVerdict`, `JudgeError`, `EnvironmentState`, `StateCapture`, semantic evaluator stubs) and delivers production-ready providers and 17+ LLM-judge evaluators. Explicit gap-closing against strands-agents/evals, langchain-ai/openevals, and google/adk-python AgentEvaluator."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Score Agent Runs with Production LLM Judges (Priority: P1)

A developer evaluating a production agent wants to grade every run against multiple quality, safety, and behavioral dimensions — helpfulness, correctness, hallucination, harmfulness, PII leakage, and task completion — using LLM judges backed by a real provider (Claude, GPT, Bedrock, etc.). They enable the desired judge feature, register the relevant evaluators with a shared `JudgeClient`, and run an eval set. Each case returns structured per-evaluator scores with reasons. The evaluator registry decides which judges apply to each case based on which case fields are populated, and quietly returns `None` for evaluators a case doesn't target.

**Why this priority**: Production LLM judges are the primary value this spec delivers — every other capability (runner upgrades, simulation, reporting) amplifies this one. Without concrete `JudgeClient` implementations, the semantic evaluator surface from 023 is inert.

**Independent Test**: Can be fully tested by running an eval set with at least one case in each evaluator family (quality, safety, RAG, agent, structured-output, code) against a live provider, asserting that every applicable evaluator returns a score with a non-empty reason, and that non-applicable evaluators return no entry.

**Acceptance Scenarios**:

1. **Given** a `JudgeClient` configured for Anthropic and a case with `expected_output` set, **When** the `CorrectnessEvaluator` runs, **Then** it returns a score in `[0.0, 1.0]` with a judge reason in details.
2. **Given** the same registry with multiple judges enabled and a case that only sets `expected_output`, **When** the registry evaluates the case, **Then** only judges whose criterion fields are populated produce entries — others are silently omitted.
3. **Given** a case paired with a retrieved-context block and `RAGGroundednessEvaluator` enabled, **When** scoring runs, **Then** groundedness is judged against that context, not against the model's training knowledge.
4. **Given** a case whose response contains PII and `PIILeakageEvaluator` is registered, **When** scoring runs, **Then** the evaluator returns a Fail with the detected entity classes in details.
5. **Given** a `JudgeClient` that transiently fails with a rate-limit error, **When** the evaluator dispatches a request, **Then** the client retries with exponential backoff and the evaluator ultimately returns a score — the case does not fail on a single throttle.
6. **Given** a `JudgeClient` that fails every attempt, **When** the evaluator exhausts retries, **Then** the evaluator returns `Score::fail()` with the error surfaced in details — the surrounding registry continues to subsequent cases.

---

### User Story 2 - Run Large Eval Sets Fast and Repeatably (Priority: P1)

A developer iterating on an agent wants to re-score a 200-case eval set after tweaking a prompt or an evaluator rubric. They run the suite with parallelism `N`, a cached task-result store, and optionally `num_runs > 1` for statistical confidence on flaky judges. The runner evaluates multiple cases concurrently, reuses cached agent invocations when inputs haven't changed, and reports per-metric means and variances.

**Why this priority**: Sequential execution + full agent re-runs on every prompt tweak makes fast iteration impractical. Parallelism, caching, and repeat-run averaging collapse the feedback loop from minutes to seconds and are table-stakes for real-world eval-driven development.

**Independent Test**: Can be fully tested by running a 20-case suite twice — first run populates the task-result cache; second run (after changing only the judge prompt, not the agent) completes in a small fraction of the first run's wall-clock time without re-invoking the agent.

**Acceptance Scenarios**:

1. **Given** a 20-case eval set and `parallelism=4`, **When** the runner executes, **Then** up to 4 cases run concurrently and the total wall-clock time is materially less than sequential execution of the same suite.
2. **Given** a cached task-result store populated by a prior run, **When** the runner executes the same eval set with unchanged cases, **Then** the agent is not invoked — cached invocations feed directly into evaluator scoring.
3. **Given** `num_runs=3` and a non-deterministic judge, **When** the runner evaluates a case, **Then** the final metric result reports a mean, the individual per-run scores, and a variance diagnostic.
4. **Given** cancellation is requested mid-run, **When** the cancellation token fires, **Then** in-flight agent calls and judge calls honor the token cooperatively and in-flight cases return a partial result with a cancellation indicator rather than hanging.
5. **Given** an `initial_session_file` is configured, **When** each case begins, **Then** the agent starts with the baseline session context loaded from that file.

---

### User Story 3 - Configure and Version Prompt Templates (Priority: P1)

A developer who wants to tune a judge's rubric, add few-shot examples, or customize its output schema does so via a configuration-only change — no evaluator code is modified. Every built-in prompt carries an explicit version suffix; bumping a prompt's version is a deliberate change that is visible in logs and reports, so historical score comparisons remain meaningful.

**Why this priority**: Prompts ARE the evaluation logic for LLM judges. Without a first-class template registry and version discipline, teams can't reason about score drift across runs, and can't safely evolve prompts over time.

**Independent Test**: Can be fully tested by overriding a built-in evaluator's prompt with a custom template and a few-shot example block, verifying the rendered prompt sent to the judge reflects the overrides, and verifying that the stored result records the prompt version used.

**Acceptance Scenarios**:

1. **Given** a built-in `CorrectnessEvaluator` with prompt `correctness_v0`, **When** the developer calls `.with_prompt(my_custom_template)`, **Then** the custom template replaces the built-in at evaluation time and the custom template's version appears in result details.
2. **Given** an evaluator configured with few-shot examples, **When** it renders the prompt for a case, **Then** the examples are injected into the prompt in the positions declared by the template.
3. **Given** a built-in prompt template with named variables (e.g. `{expected}`, `{actual}`), **When** an evaluator renders it for a case, **Then** variables are substituted from the case fields and a missing variable produces a clear compile-time or construction-time error — not a runtime string containing `{actual}` verbatim.
4. **Given** a prompt template bumped from `_v0` to `_v1`, **When** the developer consumes the new version, **Then** they opt in explicitly — the old version remains available until removed in a later release.
5. **Given** a judge configured with `use_reasoning: true`, **When** it produces a verdict, **Then** the reason field is populated and the reason text accompanies the score in all downstream reports.

---

### User Story 4 - Evaluate Multi-Turn Dialogues via Simulated Users and Tools (Priority: P2)

A developer wants to test an agent's behavior across a realistic multi-turn conversation — for example, a customer-support agent handling a frustrated user, with tools the developer hasn't wired up in their test environment. They configure an `ActorSimulator` with a user profile, goal, and turn cap, and a `ToolSimulator` that produces plausible, schema-valid responses for the tools the agent calls. The simulation drives the full dialogue to completion or turn cap, records the invocation, and then scores it with whatever evaluators are registered.

**Why this priority**: Many agent failures emerge only across multiple turns (context loss, repeated questions, goal drift). Single-prompt evaluation misses these entirely. P2 because single-prompt scoring (US1) covers the majority of test cases and this story depends on US1's evaluator family for downstream scoring.

**Independent Test**: Can be fully tested by configuring an `ActorSimulator` and `ToolSimulator`, running a 5-turn scripted scenario where the simulated user calls a goal-completion tool at turn 3, and verifying the resulting `Invocation` contains exactly 3 user turns plus agent responses with correct tool-call/tool-result pairings — then feeding that invocation into a `GoalSuccessRateEvaluator`.

**Acceptance Scenarios**:

1. **Given** an `ActorSimulator` with a profile and goal and an agent, **When** `run_multiturn_simulation` executes with `max_turns=10`, **Then** the simulator drives user turns until either the goal-completion signal fires or `max_turns` is reached.
2. **Given** a `ToolSimulator` registered for a tool and a tool schema, **When** the agent calls that tool, **Then** the simulator produces a schema-valid response and stores the call in its bounded history.
3. **Given** a `ToolSimulator` sharing state across multiple related tools via a `state_key`, **When** the agent calls one tool and then another in the same key, **Then** the second tool's response reflects state established by the first.
4. **Given** a tool schema the simulator cannot satisfy (e.g., ambiguous or contradictory), **When** the simulator generates a response, **Then** the response fails schema validation and the evaluator receives a diagnostic — not a silent malformed response.
5. **Given** a completed simulation, **When** evaluators run over the captured `Invocation`, **Then** they score it identically to an `Invocation` from a real agent run — simulation is transparent to the scoring layer.

---

### User Story 5 - Auto-Generate Diverse Test Cases from a Context Description (Priority: P2)

A developer writing evaluations for a new agent provides a context paragraph ("our agent handles refunds, shipping questions, and product recommendations for an online outdoor-gear retailer"), a task description ("verify the agent resolves the user's issue politely and cites company policy correctly"), and a desired case count. The `ExperimentGenerator` plans diverse topics, distributes cases across them, and produces an `EvalSet` with varied inputs that stress different facets of the agent.

**Why this priority**: Hand-authoring broad test coverage is tedious and biased toward the developer's imagined failure modes. Auto-generation scales coverage and surfaces edge cases the author wouldn't think of. P2 because the quality of generated cases depends on the judge prompt quality from US1 and US3.

**Independent Test**: Can be fully tested by invoking `ExperimentGenerator` with a synthetic context, `desired_count=12`, `num_topics=4`, and a `MockJudge` that returns canned case JSON — verifying 12 syntactically valid `EvalCase`s are produced spanning all 4 topics.

**Acceptance Scenarios**:

1. **Given** a context + task description + `desired_count=20` + `num_topics=5`, **When** the generator runs, **Then** it returns 20 `EvalCase`s distributed across 5 topics.
2. **Given** the agent's tool set is provided to the generator, **When** cases are generated, **Then** `expected_trajectory` fields (when `include_expected_trajectory=true`) reference only tools the agent actually has.
3. **Given** a non-deterministic judge that occasionally returns malformed JSON, **When** generation encounters a malformed response, **Then** the generator retries up to a bounded attempt count before skipping that slot — never emitting a malformed case.
4. **Given** `include_expected_output=false`, **When** cases are emitted, **Then** `expected_output` is unset on every emitted case.
5. **Given** a generation run, **When** it completes, **Then** every emitted case passes case-schema validation before being written — no malformed cases reach disk or the runner.

---

### User Story 6 - Ingest Agent Traces from External Observability Backends (Priority: P2)

A developer whose agent already emits OTel traces to Langfuse, OpenSearch, or CloudWatch wants to evaluate those traces offline without re-running the agent. They configure a `TraceProvider` for their backend, pull a session by ID, and score it with the same evaluator registry they'd use for in-process runs. Session mappers translate OpenInference / LangChain / OTel GenAI conventions into the internal `Invocation` shape.

**Why this priority**: Production eval workflows are built around whatever tracing system is already in place. Forcing teams to replay live agent runs for eval breaks that model and doubles cost. P2 because in-process evaluation (US1, US2) covers the dev-loop case; trace ingestion is the production-integration case.

**Independent Test**: Can be fully tested by recording an in-process agent run via the in-memory OTel exporter shipped with this crate, then using the `OtelInMemoryTraceProvider` + `OtelGenAiSessionMapper` to re-load it into an `Invocation`, then scoring it with the same evaluators — verifying scores match the in-process results to within floating-point equality.

**Acceptance Scenarios**:

1. **Given** an OpenInference-instrumented trace exported to an in-memory sink, **When** `OtelInMemoryTraceProvider` + `OpenInferenceSessionMapper` load it, **Then** the resulting `Invocation` contains every tool call and result from the original trace, in order, with correct arguments.
2. **Given** a trace using OTel GenAI semantic conventions v1.27, **When** `OtelGenAiSessionMapper` is configured with `GenAIConventionVersion::V1_27`, **Then** it maps attributes correctly; switching to V1_30 updates attribute mappings without code changes.
3. **Given** a session missing required attributes, **When** the mapper processes it, **Then** it returns a structured `MappingError` identifying the missing attribute — not a panic.
4. **Given** the `TraceProvider` feature for a backend is not enabled at build time, **When** the developer attempts to use it, **Then** they get a clear compile-time (feature-gated) or construction-time error — not a vague runtime failure.
5. **Given** a multi-agent swarm session, **When** `SwarmExtractor` processes its results, **Then** it produces an `Interaction` sequence the `InteractionsEvaluator` can score.

---

### User Story 7 - Emit OTel Spans for Eval Runs Themselves (Priority: P2)

An operator running evals on a schedule wants to correlate eval regressions with downstream system changes using their existing observability stack. The `EvalRunner`, when configured with `EvalsTelemetry`, emits one OTel span per case and child spans per evaluator, with standardized attributes for metric name, score, pass/fail, and eval-set ID — so a regression in `correctness` at 03:14 UTC is visible in the same dashboard as the deployment that caused it.

**Why this priority**: Evals are most valuable when their signals are pipelined into the same telemetry substrate teams already watch. P2 because the eval run returns structured results programmatically regardless; OTel emission is a production-polish feature for continuous evaluation.

**Independent Test**: Can be fully tested by running an eval set with `EvalsTelemetry` configured against the in-memory OTel exporter, asserting the expected span tree (eval-set span → per-case span → per-evaluator span) with the right attribute set.

**Acceptance Scenarios**:

1. **Given** `EvalsTelemetry` enabled, **When** `run_set` executes, **Then** one span is emitted per case with attributes for `case_id`, `eval_set_id`, overall verdict, and duration.
2. **Given** an evaluator scores a case, **When** scoring completes, **Then** a child span is emitted with attributes for `evaluator_name`, `prompt_version`, `score.value`, `score.threshold`, and `verdict`.
3. **Given** a case fails with an unrecoverable agent error, **When** the span is finalized, **Then** it records the error on the span (OTel status error + exception event) rather than silently succeeding.
4. **Given** evals are run as part of a larger trace context (e.g., inside a CI job that already has tracing enabled), **When** spans are emitted, **Then** they nest correctly under the active parent span.

---

### User Story 8 - Produce CI- and Human-Friendly Reports (Priority: P2)

A developer running evals locally sees an interactive collapsible report with color-coded pass/fail, inline judge reasons, and per-case expansion. The same eval, run in CI, emits a machine-readable JSON artifact plus a Markdown summary suitable for a PR comment. Optional HTML and LangSmith exporters serve teams with those needs.

**Why this priority**: Raw `EvalSetResult` JSON is inadequate for both the dev loop (too dense) and for PR reviews (too machine-readable). Purpose-built reporters per context are low-effort and high-impact. P2 because the core result structure from 024 is already usable; reporters are presentation polish.

**Independent Test**: Can be fully tested by running the same `EvalSetResult` through each reporter in turn and verifying that each output format parses / renders cleanly and contains the expected per-case and per-metric detail.

**Acceptance Scenarios**:

1. **Given** a completed `EvalSetResult`, **When** `RichConsoleReporter` renders it to a terminal, **Then** cases are displayed in a collapsible table with color-coded pass/fail, and per-case expand shows per-evaluator scores and reasons.
2. **Given** the same result, **When** `JsonReporter` emits an artifact, **Then** the JSON is self-contained, includes all case and metric detail, and validates against a published schema.
3. **Given** the same result, **When** `MarkdownReporter` emits a summary, **Then** the output is a valid Markdown table suitable for inline rendering in PR comments, with no terminal-only ANSI codes.
4. **Given** the `html-report` feature enabled, **When** `HtmlReporter` runs, **Then** it produces a single self-contained HTML file with no external asset dependencies.
5. **Given** the `langsmith` feature enabled and a LangSmith API token, **When** the LangSmith exporter runs, **Then** the `EvalSetResult` is pushed as a run to LangSmith with each evaluator's score attached as feedback under its configured key.

---

### User Story 9 - Migrate Legacy Formats and Integrate with Existing CI (Priority: P3)

A developer adopting `swink-agent-eval` already has `.test.json` files from an earlier framework. They point the loader at a directory, legacy schemas auto-migrate, and a shipped GitHub Actions workflow template wires PR-time and nightly eval runs into CI. A CLI entry point (`swink-eval run <set>`) provides local invocation without Rust-code integration.

**Why this priority**: Developer-experience scaffolding is important but not functionally blocking — teams can hand-write the integration points. P3 captures the polish that makes adoption friction-free.

**Independent Test**: Can be fully tested by loading a directory of legacy `.test.json` files via the legacy-format converter, asserting the resulting `EvalSet` is well-formed, then running the CLI against it and verifying identical output to the in-process API.

**Acceptance Scenarios**:

1. **Given** a directory containing legacy `.test.json` files, **When** the legacy-format converter walks the directory, **Then** each file is migrated into a valid `EvalCase` and aggregated into an `EvalSet` named for the directory.
2. **Given** a legacy file with fields not present in the current schema, **When** migration runs, **Then** unknown fields are surfaced through a `metadata` map rather than silently dropped.
3. **Given** the shipped GitHub Actions workflow templates copied into a consumer's repo, **When** a PR is opened, **Then** the PR-time workflow runs a declared eval set, posts the Markdown report as a PR comment, and sets the workflow status based on the gate.
4. **Given** the `swink-eval` CLI is invoked with `run --set path/to/set.json`, **When** it completes, **Then** stdout contains the configured reporter's output and the exit code reflects the gate result (0 pass, non-zero fail).

---

### Edge Cases

- **A judge returns a verdict with score outside `[0.0, 1.0]`**: The evaluator clamps and records a warning in details; it does not crash.
- **A judge returns structured output that fails schema validation**: Treated as `JudgeError::MalformedResponse` per 023's contract — `Score::fail()` with parse error in details, registry continues.
- **Parallelism configured higher than the provider's rate limit**: The throttle-aware retry absorbs transient 429s via exponential backoff; persistent overrun results in failed cases whose errors clearly indicate rate-limit exhaustion, not silent timeouts.
- **Cached task result exists but the case's `system_prompt` or `user_messages` changed**: The cache is keyed by a content hash of the case input; a change invalidates the cache entry and forces agent re-invocation.
- **`num_runs > 1` but one run's agent call fails**: The failing run is recorded; averaging proceeds over successful runs with a warning; if all runs fail, the case records `Verdict::Fail` with the first error in details.
- **`ActorSimulator` exceeds `max_turns` without goal completion**: The simulation terminates cleanly and the `Invocation` captures all turns; the `GoalSuccessRateEvaluator` receives the full transcript and judges accordingly (likely Fail).
- **`ToolSimulator` state bucket exceeds `max_tool_call_cache_size`**: Oldest entries are evicted FIFO; the simulator's prompts omit evicted history gracefully; no panics on overflow.
- **`ExperimentGenerator` produces a case with a circular or self-referential field**: Case-schema validation rejects it; the generator retries or omits that slot.
- **`SandboxedExecutionEvaluator` invoked on Windows**: Returns a clear `EvaluatorError::UnsupportedPlatform` immediately — no silent fallthrough, no attempt to fake execution.
- **`TraceProvider` receives a partially written trace (in progress)**: The provider returns an error indicating the session is not yet terminal; evaluators do not see a partial invocation mistaken for a complete one.
- **Two evaluators in the registry have the same `name()` but different configurations**: Registration rejects the second with a clear error; evaluator names must be unique within a registry.
- **Custom prompt template references a variable the case doesn't populate**: Construction-time (or compile-time via `prompt_template!`) rejection with the missing variable name — never silent substitution of empty strings.
- **`HtmlReporter` asked to render a result with thousands of cases**: Output remains a single self-contained file; per-case detail is collapsed by default with expand-on-click, preventing page weight from exceeding a bounded size.
- **LangSmith push fails midway through a batch**: Successfully pushed cases stay pushed; the failure is surfaced as a structured error with the count of pushed vs. failed; no partial state is persisted locally.
- **An evaluator panics inside `evaluate_async`**: Caught at the registry boundary (per 023's contract); converted to `Score::fail()` with diagnostic context; neither the case nor the run aborts.

## Requirements *(mandatory)*

### Functional Requirements

**Judge infrastructure (scope item 1)**

- **FR-001**: The system MUST provide concrete `JudgeClient` implementations for every workspace adapter (anthropic, openai, bedrock, gemini, mistral, azure, xai, ollama, proxy), each gated behind an opt-in feature flag such that the default build of `swink-agent-eval` depends on no provider.
- **FR-002**: The system MUST provide a default judge model configurable per-evaluator and per-registry, selectable without changing any prompt template.
- **FR-003**: Every `JudgeClient` MUST expose an async-first interface and a synchronous convenience wrapper suitable for blocking contexts.
- **FR-004**: Every `JudgeClient` MUST support throttling-aware retry with exponential backoff, with up to 6 attempts and a maximum backoff delay of 4 minutes, honoring cancellation cooperatively.
- **FR-005**: Every `JudgeClient` MUST support request batching with a configurable batch size (default 1, bounded upper limit) that coalesces multiple judge calls into a single provider request where the provider supports it.
- **FR-006**: The system MUST provide an in-memory prompt+model cache keyed by the hash of the prompt content and model identifier, bounded by a configurable entry count, and an optional disk-backed cache for cross-run reuse.

**Prompt-template registry (scope item 2)**

- **FR-007**: The system MUST provide a `JudgePromptTemplate` trait exposing an explicit version identifier and a render method that produces a prompt string from a context value.
- **FR-008**: Templates MUST support named variable substitution. Missing or misspelled variables MUST produce a deterministic error at template construction or evaluator construction time — never silent runtime substitution of an empty string.
- **FR-009**: The system MUST ship a built-in registry of templates organized into families (quality, safety, RAG, agent, multimodal, code, structured).
- **FR-010**: Every evaluator MUST support per-instance overrides for prompt template, few-shot examples, system prompt, output schema, and a use-reasoning flag without requiring a code change to the evaluator.
- **FR-011**: Every LLM-judge result MUST record the prompt template version used so historical comparisons can distinguish score drift caused by template changes from score drift caused by model or prompt content.

**LLM judge evaluator family (scope item 3)**

- **FR-012**: The system MUST provide quality-family evaluators: HelpfulnessEvaluator (7-level), CorrectnessEvaluator (with optional reference output), ConcisenessEvaluator (3-level), CoherenceEvaluator (5-level), ResponseRelevanceEvaluator, HallucinationEvaluator / FaithfulnessEvaluator, PlanAdherenceEvaluator, LazinessEvaluator, and GoalSuccessRateEvaluator (consumes `expected_assertion`).
- **FR-013**: The system MUST provide safety-family evaluators: HarmfulnessEvaluator / ToxicityEvaluator (binary), FairnessEvaluator, PIILeakageEvaluator, PromptInjectionEvaluator, and CodeInjectionEvaluator.
- **FR-014**: The system MUST provide RAG-family evaluators: RAGGroundednessEvaluator, RAGRetrievalRelevanceEvaluator, RAGHelpfulnessEvaluator, and EmbeddingSimilarityEvaluator (deterministic; takes an `Embedder` trait supplied by the consumer).
- **FR-015**: The system MUST provide agent / trajectory-family evaluators: TrajectoryAccuracyEvaluator (with and without reference), TaskCompletionEvaluator, UserSatisfactionEvaluator, AgentToneEvaluator, KnowledgeRetentionEvaluator, LanguageDetectionEvaluator, PerceivedErrorEvaluator, and InteractionsEvaluator (multi-agent hand-off scoring).
- **FR-016**: The system MUST provide structured-output evaluators: JsonMatchEvaluator with per-key aggregation strategies (Average, All, None) and per-key rubrics plus an `exclude_keys` filter, and JsonSchemaEvaluator (deterministic schema validation).
- **FR-017**: The system MUST provide code-family evaluators: CargoCheckEvaluator, ClippyEvaluator, CodeExtractor (markdown-fence / LLM / regex strategies), CodeLlmJudgeEvaluator, and SandboxedExecutionEvaluator (Linux/macOS only; Windows MUST fail fast with a clear unsupported-platform error).
- **FR-018**: The system MUST provide simple-family evaluators: ExactMatchEvaluator and LevenshteinDistanceEvaluator.
- **FR-019**: The system MUST provide image-safety multimodal evaluators behind a `multimodal` feature gate; audio multimodal evaluators are out of scope for this spec.
- **FR-020**: Every new evaluator MUST return `None` when the case does not set the criterion the evaluator scores — same convention as 023's built-ins.
- **FR-021**: Every new evaluator MUST be panic-isolated: panics in a judge call, a simulator step, a generator call, or a reporter renderer MUST be caught and converted to `Score::fail()` with diagnostic context, never propagating to abort the registry.

**Aggregators (scope item 4)**

- **FR-022**: The system MUST expose an `Aggregator` trait for reducing multiple `EvalMetricResult` outputs from a single evaluator run into one composite score, with built-in implementations for average, all-pass, any-pass, and weighted reductions.
- **FR-023**: Every evaluator MUST accept an optional custom aggregator override.

**Multiturn simulation (scope item 5)**

- **FR-024**: The system MUST provide an `ActorSimulator` that drives a simulated user across multiple turns via an LLM, configurable with a profile (traits, context, goal), an initial-greeting pool, a `max_turns` cap, and a goal-completion signal the simulated user can emit.
- **FR-025**: The system MUST provide a `ToolSimulator` that generates schema-valid tool responses via an LLM when real tool infrastructure is unavailable, with a `StateRegistry` that buckets state by key, a bounded previous-call history per bucket (configurable size), and shared-state semantics within a bucket.
- **FR-026**: The system MUST provide a `run_multiturn_simulation` orchestrator that drives agent ↔ actor (or agent ↔ agent) up to `max_turns` or goal-completion, returning a full `Invocation` scorable by any evaluator registered in the registry.

**Experiment generation (scope item 6)**

- **FR-027**: The system MUST provide an `ExperimentGenerator` that produces an `EvalSet` from a context description, task description, desired case count, and toggle flags for including expected output, trajectory, interactions, and metadata.
- **FR-028**: The system MUST provide a `TopicPlanner` that produces a diverse topic plan and distributes cases across topics to maximize coverage.
- **FR-029**: When an agent's tool set is provided to the generator and trajectory inclusion is enabled, generated trajectories MUST reference only tools the agent actually has.
- **FR-030**: The generator MUST validate every emitted case against the case schema before returning; malformed cases MUST never reach consumers — the generator retries up to a bounded count or omits the slot.

**Observability / trace ingestion (scope item 7)**

- **FR-031**: The system MUST provide a `TraceProvider` trait plus an always-available `OtelInMemoryTraceProvider`, with feature-gated concrete providers for OTLP-HTTP, Langfuse, OpenSearch, and CloudWatch.
- **FR-032**: The system MUST provide session mappers for OpenInference, LangChain-OTel, and the OTel GenAI semantic conventions. The GenAI mapper MUST accept a `GenAIConventionVersion` enum supporting at least v1.27, v1.30, and "experimental".
- **FR-033**: The system MUST provide an `EvaluationLevel` enum (TOOL / TRACE / SESSION) and a `TraceExtractor` that yields the appropriate granularity of input to each evaluator family.
- **FR-034**: The system MUST provide a `SwarmExtractor` and `GraphExtractor` that consume the output types from specs 040 and 039 respectively and produce input suitable for `InteractionsEvaluator`.
- **FR-035**: The system MUST provide an `EvalsTelemetry` configuration that emits OTel spans for eval runs — one per case, one per evaluator as a child — with standardized attributes including case id, eval set id, evaluator name, prompt version, score, and verdict.

**Runner upgrades (scope item 8)**

- **FR-036**: The `EvalRunner` MUST support parallel case execution with a configurable parallelism bound. Parallelism of 1 MUST be behaviorally equivalent to the current sequential implementation.
- **FR-037**: The `EvalRunner` MUST support repeating each case `num_runs` times (default 1) and averaging metric results, reporting per-run scores and a variance diagnostic.
- **FR-038**: The system MUST provide an `EvaluationDataStore` trait and a local-filesystem implementation that caches agent invocations by case name. Cache keys MUST invalidate when case inputs change.
- **FR-039**: The `EvalRunner` MUST support an `initial_session_file` parameter that loads baseline session state before each case.
- **FR-040**: Cancellation MUST propagate cooperatively through the runner to both in-flight agent calls and in-flight judge calls.

**Reporting (scope item 9)**

- **FR-041**: The system MUST provide reporters producing interactive console output, self-contained JSON artifacts, Markdown summaries, and — behind a feature gate — self-contained HTML reports.
- **FR-042**: The system MUST provide a feature-gated LangSmith export adapter that pushes an `EvalSetResult` as a LangSmith run with per-evaluator feedback attached.

**Case-model extensions (scope item 10)**

- **FR-043**: The `EvalCase` struct MUST be extended with optional fields: `expected_assertion`, `expected_interactions`, `few_shot_examples`, `attachments`, and a per-case `session_id` populated by auto-UUID default.
- **FR-044**: The system MUST provide a legacy-format converter that migrates older-schema eval files into the current `EvalSet` shape, preserving unknown fields in `metadata` rather than dropping them.

**CI and DX scaffolding (scope item 11)**

- **FR-045**: The system MUST ship GitHub Actions workflow templates covering PR-time eval runs, nightly eval runs, and report publication, plus a pre-commit configuration template.
- **FR-046**: The system MUST provide a CLI entry point invokable against an eval-set file with the same output fidelity as the in-process API.

**Cross-cutting constraints**

- **FR-047**: The default build of `swink-agent-eval` MUST NOT add any new mandatory dependencies beyond what spec 023 already requires. All new optional functionality MUST be reachable only behind explicit feature flags.
- **FR-048**: Every public evaluator MUST expose both a blocking `evaluate` entrypoint and an `evaluate_async` entrypoint. The blocking wrapper MUST be correct inside and outside a Tokio runtime.
- **FR-049**: No evaluator, simulator, generator, provider, mapper, or reporter MAY contain `unsafe` code. The entire workspace surface added by this spec MUST compile under `#![forbid(unsafe_code)]`.
- **FR-050**: The default test suite MUST NOT make live LLM calls; integration coverage MUST use `MockJudge` (from 023's testing module) or an HTTP test double. A dedicated `live-judges` feature MUST gate a smaller suite of live-provider canary tests.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A developer can run a multi-evaluator eval set against a real provider with a single feature-flag opt-in and no glue code beyond registry construction and a single `run_set` call. The happy-path integration is 10 lines of code or fewer.
- **SC-002**: Running 20 cases with parallelism 4 against a fast provider completes in under 2× the wall-clock time of running a single case, and adding `num_runs=3` does not linearly inflate that — repeat runs of the same case amortize across the parallelism budget.
- **SC-003**: Re-running an eval set after changing only a judge prompt (not the agent) reuses cached invocations and completes in a small fraction of the initial run's wall-clock time — no agent calls are made.
- **SC-004**: Switching the default judge model (e.g., Claude → GPT) requires changing a single constructor argument and MUST NOT change any prompt template — only the model rendering it.
- **SC-005**: Every judge evaluator records the prompt template version used in its result; bumping a prompt version is a deliberate opt-in change and never silent.
- **SC-006**: Multiturn simulation runs a 5-turn dialogue between an `ActorSimulator` and an agent using only `ToolSimulator` (no real tool infrastructure), producing a complete `Invocation` scorable by any registered evaluator.
- **SC-007**: `ExperimentGenerator` produces cases that pass syntactic validation 100% of the time; no malformed case ever reaches the eval runner.
- **SC-008**: An OTel-instrumented agent run stored in the in-memory exporter can be re-loaded via `TraceProvider` + session mapper and scored with the same evaluators as the in-process run, producing bitwise-identical scores for deterministic evaluators.
- **SC-009**: Every externally-exposed feature — every provider `JudgeClient`, every reporter, every `TraceProvider` — is reachable only behind an opt-in feature flag; the default build adds no mandatory dependencies versus spec 023.

## Key Entities

**Judge infrastructure**

- **JudgeClient** (from 023; consumed here): Minimal async trait for LLM-based judging. This spec ships concrete per-provider implementations behind feature flags.
- **JudgeRegistry**: Top-level configuration binding a default judge model, retry policy, batch size, and cache to an `EvaluatorRegistry`.
- **JudgePromptTemplate**: Trait for a versioned, variable-substituting prompt template that an evaluator renders for each case.
- **PromptTemplateRegistry**: Collection of built-in templates organized by family (quality, safety, RAG, agent, multimodal, code, structured).
- **JudgeCache**: In-memory LRU cache of prompt+model → verdict; optional disk-backed variant.

**LLM-judge evaluators** — one type per entry in FR-012 through FR-019. Each implements the `Evaluator` trait from 023, returning `None` when its criterion is not set on the case and otherwise rendering its prompt, dispatching via its `JudgeClient`, and aggregating verdicts into an `EvalMetricResult`.

**Aggregator**: Trait and built-in implementations (average, all-pass, any-pass, weighted) for reducing multi-output evaluator scores.

**Simulation**

- **ActorSimulator**: LLM-driven simulated conversation partner with a profile, goal, greeting pool, turn cap, and goal-completion signaling tool.
- **ActorProfile**: Named trait set, context paragraph, and goal description used to parameterize an `ActorSimulator`.
- **ToolSimulator**: LLM-driven tool stand-in with a `StateRegistry`, shared-state semantics per bucket, and schema-validated outputs.
- **StateRegistry**: Bucketed state store keyed by `state_key`, each bucket holding a bounded FIFO of previous tool calls plus arbitrary state data.
- **MultiturnSimulation**: Orchestrator binding an `ActorSimulator`, an `Agent` (real or simulated), and optional `ToolSimulator`s into a single run that produces an `Invocation`.

**Generation**

- **ExperimentGenerator**: Produces an `EvalSet` from a context + task description + target case count via an LLM.
- **TopicPlanner**: Plans diverse topics and distributes cases across them.

**Trace ingestion**

- **TraceProvider**: Trait for pulling session traces from an external observability backend and returning `Invocation`s.
- **OtelInMemoryTraceProvider**: Built-in always-available provider reading from a configurable in-memory OTel exporter.
- **SessionMapper**: Trait that converts an external trace schema (OpenInference, LangChain-OTel, OTel GenAI) into the internal `Invocation` shape.
- **GenAIConventionVersion**: Enum tagging the OTel GenAI semantic-convention variant a trace uses.
- **EvaluationLevel**: Enum (TOOL / TRACE / SESSION) indicating the granularity of input an evaluator family consumes.
- **TraceExtractor**: Strategy object that, given an `Invocation` and an `EvaluationLevel`, produces the right shape of input for each evaluator.
- **SwarmExtractor / GraphExtractor**: Adapters converting multi-agent swarm and graph result types (from specs 040 and 039) into `Interaction` sequences.

**Runner**

- **EvalRunner** (extended from 023/024): Gains `with_parallelism(n)`, `num_runs`, `initial_session_file`, and cooperative cancellation across agent and judge calls.
- **EvaluationDataStore**: Trait for caching agent invocations by content-hashed case input; `LocalFileTaskResultStore` is the built-in filesystem implementation.
- **EvalsTelemetry**: Configuration for emitting OTel spans per eval run, per case, per evaluator.

**Reporting**

- **EvaluationReport**: Aggregate type with structured per-case reasoning, metric breakdown, and failure narrative; input to every reporter.
- **Reporter**: Trait implemented by `RichConsoleReporter`, `JsonReporter`, `MarkdownReporter`, `HtmlReporter`, and the `LangSmithExporter`.

**Case-model extensions** on `EvalCase`: `expected_assertion` (goal-completion criteria), `expected_interactions` (multi-agent hand-off topology), `session_id` (auto-UUID), `few_shot_examples` (per-case judge examples), `attachments` (multimodal data refs).

**CLI**: A `swink-eval` binary exposing `run`, `report`, `gate`, and `migrate` subcommands against eval sets on disk.

## Assumptions

- 023's `JudgeClient`, `JudgeVerdict`, `JudgeError`, `EnvironmentState`, `StateCapture`, and semantic-evaluator stubs are frozen. This spec consumes them unchanged.
- 024's `FsEvalStore`, `GateConfig`, and `AuditedInvocation` remain the persistence and CI-gate substrate; runner upgrades extend rather than replace them.
- Provider-backed `JudgeClient` implementations live in a single `eval-judges` workspace crate with per-provider feature flags. Each feature re-exports a `<Provider>JudgeClient` type that wraps the corresponding adapter without polluting the adapter crates with eval-layer concerns. (Alternative: per-adapter sub-modules behind features inside `eval`; the chosen structure keeps adapter crates pure.)
- The default judge model is Anthropic's current flagship Sonnet. Teams that want a different default swap it via one constructor call.
- "No new mandatory dependencies" means the default build of `swink-agent-eval` adds no new transitive crates beyond what spec 023 already pulls in. New crates used by this spec (e.g. a retry crate, a string-similarity crate, terminal-rendering crates, an HTML templating crate) live under feature flags.
- `SandboxedExecutionEvaluator` uses OS-level resource limits (process groups, rlimits) rather than container isolation. It is deliberately unsupported on Windows; teams on Windows can still run every other evaluator, and the CI matrix explicitly includes Linux and macOS targets only for this evaluator.
- `EmbeddingSimilarityEvaluator` ships with an `Embedder` trait and no default implementation; consumers supply their own embedder (e.g. local model or provider call) so we don't bundle a default embedding provider.
- Audio multimodal evaluators are deferred to a later spec; only image-safety prompts ship in this spec's multimodal feature.
- Prompt-template versioning follows a `_v0`, `_v1` suffix convention; a bumped version is an intentional semver-impacting change and the old version remains accessible until removed in a later release.
- `num_runs` averaging is arithmetic mean; a future spec may add median, trimmed mean, or percentile reductions if real-world judge variance warrants them.
- The CLI and workflow templates target teams that use GitHub Actions. Support for other CI systems (GitLab, CircleCI, etc.) is a future enhancement; the core library surface is CI-agnostic so third parties can write their own templates.
- LangSmith integration pushes results over their HTTP API. The integration uses a token-based auth model and does not require any LangSmith-side project preconfiguration beyond project creation.

## Dependencies

- **Spec 023** (eval-trajectory-matching): frozen trait surface — `JudgeClient`, `JudgeVerdict`, `JudgeError`, `EnvironmentState`, `StateCapture`, `SemanticToolSelectionEvaluator` stub, `SemanticToolParameterEvaluator` stub, `EnvironmentStateEvaluator`, `MockJudge` test double. This spec wires real provider implementations behind those traits.
- **Spec 024** (eval-runner-governance): `FsEvalStore`, `GateConfig`, `AuditedInvocation`, current sequential runner. This spec extends the runner with parallelism, `num_runs`, task-result caching, and reporters.
- **Spec 010** (loop-policies-observability): policy slot model and OTel span-emission patterns. `EvalsTelemetry` mirrors those patterns.
- **Spec 031** (policy-slots): `BudgetPolicy` contract. Judge calls and simulator calls respect the configured `BudgetPolicy` for the surrounding run.
- **Spec 039** (multi-agent-patterns): graph multi-agent result types consumed by `GraphExtractor`.
- **Spec 040** (agent-transfer-handoff): swarm multi-agent result types consumed by `SwarmExtractor`.
- **Specs 011–020** (adapter family): the per-provider adapter crates wrapped by each `<Provider>JudgeClient` implementation.

## References

**Internal**:
- `specs/023-eval-trajectory-matching/spec.md` — frozen trait surface this spec consumes.
- `specs/024-eval-runner-governance/spec.md` — runner, store, gate, audit substrate extended here.
- `docs/HLD.md` § Evaluation Layer — architecture context.

**External**:
- `strands-agents/evals` (cloned at `~/Development/strands-evals`, ~11.5K LOC across 100 files) — primary source for `ActorSimulator`/`ToolSimulator` design, `ExperimentGenerator`/`TopicPlanner`, `TraceProvider`/`SessionMapper` architecture, multi-agent extractors, GenAI convention support, `EvaluationDataStore` caching pattern.
- `langchain-ai/openevals` — primary source for the prompt registry shape; prebuilt prompt families across quality, safety, RAG, agent, multimodal, structured, and code; `JsonMatchEvaluator` with per-key rubrics; multiturn simulation orchestrator surface; sandboxed code-execution model (E2B → Rust tempdir analog); configurable output schemas, feedback-key naming, and use-reasoning flag.
- `google/adk-python` `AgentEvaluator` — primary source for `num_runs` averaging, `EvalConfig`-threshold model, `.test.json` directory loading with recursive walk, `initial_session_file` persistent context, legacy schema migration, and tabular report style.
