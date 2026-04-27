# Feature Specification: Eval-Driven Self-Improvement Loop

**Feature Branch**: `044-self-improvement-loop`
**Created**: 2026-04-27
**Status**: Draft
**Input**: A new workspace crate `swink-agent-evolve` that implements a closed-loop optimization cycle for agent system prompts, tool descriptions, and skill definitions. The loop runs the existing eval suite against a target configuration, diagnoses weak points, generates candidate improvements via mutation strategies, re-evaluates candidates, gates on quality thresholds, and persists accepted improvements with a full audit trail. Builds on swink-agent-eval's EvalRunner, EvalCase, JudgeClient, and Reporter infrastructure (specs 023, 024, 043). Feature-gated, cost-bounded, deterministic where possible, with JSONL audit manifests.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Run a Baseline Evaluation Against a Configuration (Priority: P1)

An agent developer has a system prompt, a set of tool schemas, and a model preset that define their agent's current behavior. They want to establish a baseline quality score before attempting any optimization. They create an `OptimizationTarget` describing what can be mutated and an `OptimizationConfig` with their eval set and budget. They run a single baseline cycle that executes the eval suite and produces a scored snapshot of the current configuration — no mutations applied. The developer inspects the per-case results to understand where the agent performs well and where it struggles.

**Why this priority**: Without a baseline, no optimization can be measured. This is the entry point to the entire loop and validates that the existing eval infrastructure integrates correctly.

**Independent Test**: Can be tested by constructing an `OptimizationTarget` with a known system prompt, running `EvolutionRunner::baseline()` with a small eval set, and verifying the returned `BaselineSnapshot` contains per-case scores that match running the eval suite directly.

**Acceptance Scenarios**:

1. **Given** an `OptimizationTarget` with a system prompt and tool schemas, **When** `baseline()` is called with a valid eval set and agent factory, **Then** a `BaselineSnapshot` is returned containing per-case scores, aggregate score, and the original configuration.
2. **Given** a baseline run, **When** all eval cases pass, **Then** the aggregate score is the arithmetic mean of individual case scores (equal weight per case).
3. **Given** a baseline run, **When** some eval cases fail, **Then** each failure includes the evaluator name, score, and details from the eval metric result.
4. **Given** a baseline run that exceeds the configured cost budget, **Then** the run halts early and returns a partial snapshot with a budget-exceeded indicator.

---

### User Story 2 - Diagnose Weak Points from Eval Results (Priority: P1)

An agent developer has a baseline snapshot showing mixed results — some eval cases pass, others fail or score poorly. They run the diagnosis phase to identify actionable patterns: which eval cases score below threshold, which tools are selected incorrectly, and whether the same failure pattern recurs across multiple cases. The diagnosis produces a ranked list of improvement opportunities, each tied to a specific component of the configuration (system prompt section, tool description, parameter schema).

**Why this priority**: Diagnosis determines where mutations should focus. Without it, mutations are random and wasteful.

**Independent Test**: Can be tested by constructing a `BaselineSnapshot` with known failure patterns (e.g., two cases failing on the same tool selection) and verifying the `Diagnoser` identifies the correct weak points and ranks them by impact.

**Acceptance Scenarios**:

1. **Given** a baseline where three eval cases fail with low trajectory match scores on the same tool, **When** diagnosis runs, **Then** a `WeakPoint` is produced identifying that tool description as the highest-priority improvement target.
2. **Given** a baseline where one case fails on response quality but passes on trajectory, **When** diagnosis runs, **Then** a `WeakPoint` is produced pointing to the system prompt with the relevant eval case details.
3. **Given** a baseline where all cases pass with scores above 0.9, **When** diagnosis runs, **Then** the weak point list is empty and the diagnosis reports no actionable improvements.
4. **Given** a baseline with both tool selection mismatches and prompt quality failures, **When** diagnosis runs, **Then** weak points are ranked by the number of affected cases times the severity of the score gap.

---

### User Story 3 - Generate Candidate Mutations (Priority: P1)

An agent developer has a list of diagnosed weak points. They run the mutation phase to generate candidate improvements for the highest-priority weak points. The mutator supports three strategies: LLM-guided rewrite (sends the failing trace and criteria to a judge model for a suggested improvement), template-based substitution (swaps phrasing patterns from a mutation library), and ablation (removes or simplifies sections to test their necessity). Each strategy produces one or more candidate configurations that differ from the original only in the targeted component.

**Why this priority**: Mutations are the core mechanism for improvement. Without candidate generation, the loop has nothing to evaluate.

**Independent Test**: Can be tested by providing a weak point targeting a system prompt section and verifying each mutation strategy produces at least one candidate that differs from the original in the expected way. LLM-guided strategy can be tested with a mock JudgeClient that returns a fixed rewrite.

**Acceptance Scenarios**:

1. **Given** a weak point targeting a tool description with an LLM-guided strategy, **When** the mutator runs, **Then** it sends the failing trace, original description, and eval criteria to the judge model and returns the rewritten description as a candidate.
2. **Given** a weak point targeting a system prompt section with a template-based strategy, **When** the mutator runs, **Then** it produces candidates by swapping phrasing patterns (e.g., imperative to declarative, verbose to concise) from the built-in mutation library.
3. **Given** a weak point targeting a system prompt section with an ablation strategy, **When** the mutator runs, **Then** it produces a candidate with the targeted section removed and another with the section simplified to its first sentence.
4. **Given** a mutation that exceeds the configured budget for LLM calls in this cycle, **When** the mutator runs, **Then** it stops generating candidates and returns what it has with a budget-exhausted flag.
5. **Given** a deterministic seed configured on the mutation engine, **When** the same weak point and strategy are run twice, **Then** template-based and ablation mutations produce identical candidates.

---

### User Story 4 - Evaluate Candidates Against the Baseline (Priority: P1)

An agent developer has a set of candidate configurations generated by the mutator. They run the evaluation phase to score each candidate against the same eval set used for the baseline. The evaluator produces per-case scores for each candidate so they can be compared directly against the baseline.

**Why this priority**: Without re-evaluation, there is no way to know whether a mutation improved or regressed performance.

**Independent Test**: Can be tested by providing two candidate configurations (one better, one worse than baseline) and verifying the evaluator correctly scores both and produces comparable results.

**Acceptance Scenarios**:

1. **Given** a candidate configuration and the same eval set used for the baseline, **When** the candidate is evaluated, **Then** per-case scores are produced using the same evaluators as the baseline.
2. **Given** multiple candidates, **When** evaluated with parallelism enabled, **Then** candidates are evaluated concurrently up to the configured parallelism limit.
3. **Given** a candidate evaluation that exceeds the cycle budget, **Then** remaining candidates are skipped and partial results are returned.
4. **Given** a candidate whose agent factory fails to create an agent for a specific case, **Then** that case is scored as a failure (score 0.0) without crashing the evaluation of other cases.

---

### User Story 5 - Gate Acceptance on Quality Thresholds (Priority: P1)

An agent developer has scored candidates and a baseline. They run the acceptance gate to determine which candidates should be accepted. A candidate is accepted only if its aggregate score improves over the baseline by at least the configured threshold AND it does not regress any eval case marked as P1. The gate produces a ranked list of accepted candidates and a list of rejected candidates with rejection reasons.

**Why this priority**: The gate prevents regressions — without it, mutations that improve one area while breaking another would be accepted.

**Independent Test**: Can be tested by providing a baseline and candidates with known scores, including one that improves aggregate but regresses a P1 case, and verifying it is rejected.

**Acceptance Scenarios**:

1. **Given** a candidate that improves aggregate score by 0.05 (above the 0.01 default threshold) and does not regress any P1 case, **When** the gate runs, **Then** the candidate is accepted.
2. **Given** a candidate that improves aggregate score but regresses one P1 eval case from pass to fail, **When** the gate runs, **Then** the candidate is rejected with reason "P1 regression" identifying the regressed case.
3. **Given** a candidate that improves aggregate score by 0.005 (below the 0.01 default threshold), **When** the gate runs, **Then** the candidate is rejected with reason "improvement below threshold."
4. **Given** multiple accepted candidates for the same target component, **When** the gate ranks them, **Then** the candidate with the highest aggregate improvement is ranked first.
5. **Given** a custom acceptance threshold of 0.10, **When** a candidate improves by 0.08, **Then** it is rejected as below threshold.

---

### User Story 6 - Persist Accepted Improvements with Audit Trail (Priority: P1)

An agent developer has accepted candidates from the gate. They run the persistence phase to write the improved configuration to a versioned output directory with a full audit trail. The trail records the original configuration, each mutation applied, before and after scores, the mutation strategy used, and the accept/reject decision for every candidate evaluated in the cycle. The developer can replay the audit to understand why each change was made.

**Why this priority**: Without persistence and audit, accepted improvements are lost when the process exits. The audit trail provides reproducibility and accountability.

**Independent Test**: Can be tested by running a complete cycle with a mock judge and small eval set, then reading back the output directory and verifying it contains the improved configuration, the JSONL manifest, and all expected audit fields.

**Acceptance Scenarios**:

1. **Given** an accepted candidate, **When** persisted, **Then** the output directory contains the improved configuration (system prompt, tool schemas) in a versioned subdirectory named by cycle number and timestamp.
2. **Given** a completed cycle, **When** the manifest is read, **Then** each entry is a valid JSON object containing: cycle_id, timestamp, target_component, original_value, mutated_value, strategy, baseline_score, candidate_score, verdict (accepted/rejected), and rejection_reason (if applicable).
3. **Given** multiple accepted candidates across different target components, **When** persisted, **Then** each component's improvement is written separately and the manifest records all of them.
4. **Given** a cycle where all candidates were rejected, **When** persisted, **Then** the manifest still records all evaluated candidates with their rejection reasons, but no improved configuration is written.
5. **Given** two consecutive cycles, **When** the second cycle runs, **Then** it uses the output of the first cycle as its baseline configuration (if one was accepted), and the output directory contains both cycle subdirectories.

---

### User Story 7 - Run the Full Optimization Loop (Priority: P2)

An agent developer wants to run the complete loop end-to-end: baseline, diagnose, mutate, evaluate, gate, persist. They configure an `EvolutionRunner` with their target, eval set, agent factory, budget, and mutation strategies. They call `run_cycle()` which executes all phases in sequence and returns a `CycleResult` summarizing what changed. The developer can run multiple cycles to iteratively improve the configuration.

**Why this priority**: The end-to-end loop is the primary user interface, but each phase must work independently first. This story validates the integration.

**Independent Test**: Can be tested end-to-end with a mock agent factory and small eval set, verifying that a known-weak system prompt is improved after one cycle.

**Acceptance Scenarios**:

1. **Given** a fully configured `EvolutionRunner`, **When** `run_cycle()` is called, **Then** it executes baseline → diagnose → mutate → evaluate → gate → persist in sequence and returns a `CycleResult`.
2. **Given** a `CycleResult` with accepted improvements, **When** `run_cycle()` is called again, **Then** the second cycle uses the improved configuration as its baseline.
3. **Given** a cycle that exhausts its budget during the mutation phase, **When** the cycle completes, **Then** the `CycleResult` indicates partial completion with the phases that ran and those that were skipped.
4. **Given** a configured maximum of 5 cycles, **When** `run_cycles(max_cycles)` is called, **Then** it runs up to 5 cycles, stopping early if no improvements are found in a cycle.

---

### User Story 8 - Inspect Optimization History (Priority: P3)

An agent developer has run several optimization cycles and wants to review the history: which mutations were tried, which were accepted, and how scores progressed over time. They load the output directory and iterate over cycle manifests to build a timeline. The JSONL format makes it easy to filter, sort, and analyze with standard tools.

**Why this priority**: History inspection is valuable for understanding optimization trajectories but is not required for the core loop to function.

**Independent Test**: Can be tested by running three cycles, then loading the manifests and verifying the timeline shows monotonically increasing (or stable) aggregate scores.

**Acceptance Scenarios**:

1. **Given** an output directory with three cycle subdirectories, **When** manifests are loaded, **Then** they are ordered by cycle number and each contains complete audit records.
2. **Given** a manifest, **When** deserialized, **Then** every entry round-trips through serde without data loss.
3. **Given** a developer using `jq` or similar tools, **When** filtering the manifest for accepted entries only, **Then** the standard JSONL format works without custom tooling.

---

### Edge Cases

- What happens when the eval set is empty? `baseline()` returns an error — at least one eval case is required.
- What happens when the mutation budget is zero? The mutator returns zero candidates and the cycle completes with no improvements found.
- What happens when the judge model is unavailable during LLM-guided mutation? The mutation fails gracefully with a `MutationError::JudgeUnavailable` and the strategy is skipped for that weak point. Other strategies still run.
- What happens when a mutation produces a candidate identical to the original? The candidate is deduplicated and not evaluated.
- What happens when all three mutation strategies produce zero candidates for a weak point? The weak point is skipped with a diagnostic message in the cycle result.
- What happens when the output directory already contains results from a previous cycle? The new cycle creates a new versioned subdirectory; existing cycles are not modified.
- What happens when a mutation strategy panics? The panic is caught (consistent with the project's panic-isolation policy), the strategy is skipped, and the error is recorded in the manifest.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The crate MUST be a new workspace member named `swink-agent-evolve` that depends on `swink-agent` and `swink-agent-eval` public APIs only.
- **FR-002**: The crate MUST declare `#[forbid(unsafe_code)]` at the crate root. Its own `Cargo.toml` MUST define feature gates (e.g., `otel`) consistent with the workspace pattern. The crate is an optional workspace member — consumers depend on it explicitly.
- **FR-003**: `OptimizationTarget` MUST describe the mutable components: system prompt (as a string), tool schemas (as a `Vec<ToolSchema>`), and optionally named sections within the system prompt that can be independently mutated.
- **FR-004**: `OptimizationConfig` MUST specify: the eval set to use, the mutation strategies to apply, the acceptance threshold (default 0.01), the cost budget for the cycle, the parallelism limit for candidate evaluation, and an optional deterministic seed. The agent factory is a separate parameter to the `EvolutionRunner` constructor, not part of the config.
- **FR-005**: `EvolutionRunner::baseline()` MUST run the configured eval set against the original configuration using the existing `EvalRunner` and return a `BaselineSnapshot` containing per-case `EvalCaseResult`s and an aggregate score.
- **FR-006**: `Diagnoser::diagnose()` MUST analyze a `BaselineSnapshot` and produce a ranked `Vec<WeakPoint>`, where each `WeakPoint` identifies: the target component (prompt section or tool schema), the affected eval cases, the scoring gap (threshold minus actual), and a severity rank.
- **FR-007**: Weak points MUST be ranked by `affected_case_count * mean_score_gap`, descending.
- **FR-008**: `Mutator` MUST support three strategies: `LlmGuided`, `TemplateBased`, and `Ablation`. Each strategy implements a `MutationStrategy` trait with `fn mutate(target: &str, context: &MutationContext) -> Result<Vec<Candidate>, MutationError>`.
- **FR-009**: `LlmGuided` strategy MUST send the failing trace, original value, eval criteria, and score to the configured `JudgeClient` with a structured prompt requesting an improved version. The response is parsed as the candidate value.
- **FR-010**: `TemplateBased` strategy MUST apply phrasing transformations from a built-in mutation library (imperative↔declarative, verbose↔concise, restructuring, synonym substitution). The library MUST be extensible via user-provided templates.
- **FR-011**: `Ablation` strategy MUST produce at minimum two candidates per target: one with the section removed entirely and one with the section simplified to its first sentence or first 50 words (whichever is shorter).
- **FR-012**: All mutation strategies MUST respect the configured cost budget. LLM-guided mutations MUST track token usage and stop when the budget is exhausted. Template-based and ablation mutations have zero LLM cost.
- **FR-013**: Candidates MUST be deduplicated before evaluation — if two strategies produce identical output, only one is evaluated. Candidates whose mutated value is identical to the original value MUST also be filtered out.
- **FR-014**: `CandidateEvaluator` MUST run the eval set against each candidate configuration using `EvalRunner`, reusing the same evaluator registry and agent factory as the baseline.
- **FR-015**: `AcceptanceGate` MUST accept a candidate only if: (a) aggregate score improves by at least the configured threshold, AND (b) no eval case with P1 priority regresses from pass to fail. P1 priority is determined by the `EvalCase` metadata field `priority` (default: all cases are P1).
- **FR-016**: `AcceptanceGate` MUST produce an `AcceptanceResult` containing accepted candidates (ranked by improvement) and rejected candidates (each with a rejection reason enum: `BelowThreshold`, `P1Regression { case_id }`, `NoImprovement`). When multiple candidates target the same component, only the top-ranked is marked for persistence; others receive verdict `AcceptedNotApplied`.
- **FR-017**: `CyclePersister` MUST write accepted configurations to a versioned output directory: `{output_root}/cycle-{number:04}-{iso8601-timestamp}/` (zero-padded cycle number, ISO 8601 timestamp).
- **FR-018**: `CyclePersister` MUST write a JSONL manifest (`manifest.jsonl`) where each line is a JSON object containing: `cycle_id`, `timestamp`, `target_component`, `original_value`, `mutated_value`, `strategy`, `baseline_score`, `candidate_score`, `verdict`, and `rejection_reason`.
- **FR-019**: `EvolutionRunner::run_cycle()` MUST execute baseline → diagnose → mutate → evaluate → gate → persist in sequence, propagating budget across phases and returning a `CycleResult`.
- **FR-020**: `EvolutionRunner::run_cycles(max: usize)` MUST run up to `max` cycles, feeding each cycle's accepted output as the next cycle's input. It MUST stop early if a cycle produces no accepted improvements.
- **FR-021**: All mutation strategy panics MUST be caught and recorded as `MutationError::Panic` in the manifest without crashing the cycle.
- **FR-022**: When a deterministic seed is configured, `TemplateBased` and `Ablation` strategies MUST produce identical candidates for identical inputs.
- **FR-023**: `OptimizationConfig` MUST include configurable hard caps: `max_weak_points` (default 5) limiting how many weak points are processed per cycle, and `max_candidates_per_strategy` (default 3) limiting how many candidates each strategy generates per weak point. The cost budget remains an independent soft ceiling applied across all phases.
- **FR-024**: Each phase of the optimization cycle (baseline, diagnose, mutate, evaluate, gate, persist) MUST emit structured tracing spans when the `otel` feature is enabled, consistent with the eval runner's instrumented evaluation pattern. Spans MUST include phase name, cycle number, and duration.

### Key Entities

- **OptimizationTarget**: The mutable configuration being optimized — system prompt (with optional named sections), tool schemas, and model preset metadata.
- **WeakPoint**: A diagnosed improvement opportunity — links a target component to specific failing eval cases with a severity rank.
- **Candidate**: A mutated configuration variant — contains the modified value, the strategy that produced it, and the target component identifier.
- **CycleResult**: The outcome of one optimization cycle — baseline scores, candidates evaluated, accepted improvements, rejected candidates, total cost, and completion status.
- **Manifest Entry**: An audit record for a single candidate — captures the full before/after state, scores, strategy, and accept/reject decision.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A complete optimization cycle (baseline + diagnose + mutate + evaluate + gate + persist) executes successfully against a test eval set with at least 5 cases and produces a valid manifest.
- **SC-002**: Given a system prompt with a known weakness (e.g., missing instruction that causes a specific eval case to fail), the LLM-guided mutation strategy produces a candidate that fixes the failing case within 3 cycles.
- **SC-003**: The acceptance gate correctly rejects candidates that regress P1 cases in 100% of test scenarios — zero regressions slip through.
- **SC-004**: Template-based and ablation mutations produce identical results when given the same seed, input, and weak point.
- **SC-005**: Cost tracking is accurate within 5% of the actual LLM spend for a cycle, as verified against eval runner usage reports.
- **SC-006**: The manifest JSONL is valid — every line deserializes without error and contains all required fields with non-null values.
- **SC-007**: Strategy panics are caught and the cycle completes — verified by injecting a panicking mock strategy and confirming the manifest records the panic without crashing.
- **SC-008**: Multi-cycle runs with `run_cycles(3)` show monotonically non-decreasing aggregate scores when improvements are found.

## Clarifications

### Session 2026-04-27

- Q: How is the aggregate score computed — equal weight, priority-weighted, or evaluator-weighted? → A: Equal weight — every eval case contributes equally to the aggregate score. P1 regression protection is handled independently by the acceptance gate (FR-015b), so priority does not need to be mixed into the aggregate.
- Q: Should there be hard caps on weak points processed and candidates generated per cycle? → A: Yes — default max 5 weak points per cycle, max 3 candidates per strategy per weak point. Both configurable via `OptimizationConfig`.
- Q: When multiple candidates are accepted for the same target component, which is persisted? → A: Top-ranked only — the best candidate per component is persisted. Lower-ranked accepted candidates are recorded in the manifest with verdict "accepted-not-applied" for audit purposes but not written to the output configuration.
- Q: Should the evolve crate emit structured tracing spans for observability? → A: Yes — emit tracing spans per phase (baseline, diagnose, mutate, evaluate, gate, persist), feature-gated behind `otel` consistent with the existing eval runner pattern.

## Assumptions

- The existing `EvalRunner`, `EvalCase`, `Evaluator`, `JudgeClient`, and `Reporter` APIs from specs 023 and 024 are stable and publicly re-exported from `swink-agent-eval`.
- `EvalCase` metadata supports a `priority` field (string or enum) that can be read to identify P1 cases. If absent, all cases default to P1.
- The `JudgeClient` trait from spec 023/043 is used for LLM-guided mutations. The evolve crate does not implement its own LLM calling — it reuses judge infrastructure.
- The `AgentFactory` trait from `swink-agent-eval` is used to create agents for both baseline and candidate evaluation. The evolve crate wraps the factory to swap configurations.
- Cost tracking uses the `Cost` and `Usage` types from the existing eval results. The evolve crate aggregates costs across phases but does not implement its own token counting.
- The output directory is on a local filesystem. Remote storage backends are out of scope.
- System prompt sections are delimited by markdown headers (e.g., `## Section Name`) or user-defined markers. The section parser is best-effort — unstructured prompts are treated as a single section.
