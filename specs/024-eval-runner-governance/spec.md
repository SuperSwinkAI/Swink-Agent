# Feature Specification: Eval: Runner, Scoring & Governance

**Feature Branch**: `024-eval-runner-governance`
**Created**: 2026-03-20
**Status**: Draft
**Input**: Evaluation orchestration pipeline. EvalRunner, Evaluator trait and EvaluatorRegistry with defaults, Score types and aggregation, data-file-based eval case definitions, BudgetEvaluator for post-hoc scoring, EvalStore trait and filesystem-based persistence, GateConfig for CI/CD gating (pass-rate, cost, duration thresholds), AuditedInvocation with cryptographic hash chains for tamper detection. References: HLD Evaluation Layer, EVAL.md (Observability, Production Readiness).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Run an Evaluation Suite (Priority: P1)

An evaluator defines a suite of test cases in a structured data file. Each case specifies a prompt, expected behavior (golden path, response criteria), and optional metadata. The eval runner executes each case against the agent, collects trajectories, applies evaluators, and produces a summary report with per-case and aggregate scores. The evaluator reviews the report to understand how the agent performed across the suite.

**Why this priority**: The eval runner is the orchestration layer that ties everything together — without it, evaluation requires manual case-by-case execution.

**Independent Test**: Can be tested by defining a suite of three cases in a data file, running the suite, and verifying the report contains scores for all three cases plus aggregate results.

**Acceptance Scenarios**:

1. **Given** a data file with multiple eval cases, **When** the runner executes the suite, **Then** each case is run against the agent and produces individual scores.
2. **Given** a completed suite run, **When** the report is generated, **Then** it includes per-case scores and aggregate scores (mean, min, max).
3. **Given** a case that causes the agent to fail (e.g., runtime error), **When** the runner encounters it, **Then** the failure is recorded for that case and the suite continues with remaining cases.
4. **Given** a suite with no cases, **When** run, **Then** the runner returns an empty report (not an error).

---

### User Story 2 - Gate Deployments on Evaluation Results (Priority: P1)

A CI/CD pipeline runs an evaluation suite before deployment. The gate configuration specifies thresholds: minimum pass rate, maximum cost, and maximum duration. After the suite completes, the gate evaluates the results against the thresholds and produces a pass/fail decision. If the gate fails, the pipeline blocks deployment and reports which thresholds were violated. The team is protected from deploying regressions.

**Why this priority**: Deployment gating is the primary production use case for evaluation — it turns eval from a development tool into a safety mechanism.

**Independent Test**: Can be tested by running a suite, configuring a gate with specific thresholds, and verifying the gate passes when results meet thresholds and fails when they do not.

**Acceptance Scenarios**:

1. **Given** eval results that meet all gate thresholds, **When** the gate evaluates, **Then** it returns a pass decision.
2. **Given** eval results below the minimum pass rate, **When** the gate evaluates, **Then** it returns a fail decision identifying the pass-rate violation.
3. **Given** eval results exceeding the maximum cost threshold, **When** the gate evaluates, **Then** it returns a fail decision identifying the cost violation.
4. **Given** eval results exceeding the maximum duration threshold, **When** the gate evaluates, **Then** it returns a fail decision identifying the duration violation.
5. **Given** multiple threshold violations, **When** the gate evaluates, **Then** all violations are reported (not just the first).

---

### User Story 3 - Register and Compose Evaluators (Priority: P2)

An evaluator wants to apply multiple evaluation strategies to each test case: trajectory matching, efficiency scoring, response matching, and custom domain-specific checks. They register evaluators in a registry, and the runner applies all registered evaluators to each case. The evaluator can also define custom evaluator implementations and add them to the registry alongside the built-in defaults.

**Why this priority**: The registry pattern enables extensibility and composition, but a single hardcoded evaluator would still allow basic evaluation.

**Independent Test**: Can be tested by registering three evaluators (two built-in, one custom), running a case, and verifying all three produce scores that appear in the result.

**Acceptance Scenarios**:

1. **Given** a registry with default evaluators, **When** a case is evaluated, **Then** all default evaluators are applied and their scores are included.
2. **Given** a custom evaluator registered alongside defaults, **When** a case is evaluated, **Then** the custom evaluator's score appears alongside the built-in scores.
3. **Given** an evaluator that fails during execution, **When** the case is evaluated, **Then** the failure is recorded and other evaluators still run.

---

### User Story 4 - Persist and Retrieve Evaluation Results (Priority: P2)

An evaluator runs suites over time and wants to compare results across runs. The eval store persists each suite run's results to the filesystem. The evaluator can list past runs, load a specific run's results, and compare scores across runs to detect regressions or improvements. Results are stored in a structured, human-readable format.

**Why this priority**: Persistence enables trend analysis and regression detection, but evaluation provides value even without historical comparisons.

**Independent Test**: Can be tested by running a suite, saving results, loading them back, and verifying the loaded results match the originals.

**Acceptance Scenarios**:

1. **Given** a completed suite run, **When** results are saved, **Then** they are persisted to the filesystem in a structured format.
2. **Given** previously saved results, **When** loaded by run identifier, **Then** the loaded results are identical to the originals.
3. **Given** multiple saved runs, **When** the run list is requested, **Then** all runs are returned with their metadata (timestamp, suite name, aggregate scores).
4. **Given** a request to load a non-existent run, **When** attempted, **Then** a clear error is returned.

---

### User Story 5 - Produce Tamper-Evident Audit Trails (Priority: P3)

A compliance officer needs to verify that evaluation results have not been modified after the fact. Each invocation in the audit trail includes a cryptographic hash that chains to the previous invocation, creating a tamper-evident log. If any record is altered, the hash chain breaks and the tampering is detectable. The compliance officer can verify the integrity of any evaluation run.

**Why this priority**: Audit trails are a governance requirement for production environments but are not needed for development-time evaluation.

**Independent Test**: Can be tested by creating an audit trail, verifying the hash chain is valid, then modifying one record and verifying the chain breaks.

**Acceptance Scenarios**:

1. **Given** an evaluation run, **When** the audit trail is generated, **Then** each invocation record includes a cryptographic hash chaining to the previous record.
2. **Given** a valid audit trail, **When** integrity is verified, **Then** verification passes.
3. **Given** an audit trail with a tampered record, **When** integrity is verified, **Then** verification fails and identifies the point of tampering.
4. **Given** the first record in a chain, **When** inspected, **Then** it chains to a known initial value (genesis hash).

---

### User Story 6 - Score Resource Budget Compliance (Priority: P2)

An evaluator wants to check whether agent runs stayed within acceptable resource budgets after the fact. The budget evaluator scores each run based on token usage, cost, and duration relative to configured limits. Runs that exceed budgets receive lower scores. The evaluator uses this to identify test cases where the agent is wasteful.

**Why this priority**: Budget scoring provides actionable optimization data, but the core eval pipeline works without it.

**Independent Test**: Can be tested by providing runs with known token/cost/duration values, configuring budget limits, and verifying the scores reflect over/under-budget status.

**Acceptance Scenarios**:

1. **Given** a run within all budget limits, **When** scored, **Then** the budget score is 1.0.
2. **Given** a run exceeding the token budget, **When** scored, **Then** the score is penalized proportionally to the overrun.
3. **Given** a run exceeding multiple budget dimensions, **When** scored, **Then** each violation contributes to the penalty.

---

### Edge Cases

- What happens when an eval case data file is malformed or missing required fields?
- How does the runner handle a case where the agent never terminates (infinite loop)?
- What happens when the gate configuration specifies no thresholds (empty config)?
- How does score aggregation handle cases where some evaluators produce no score (e.g., skipped)?
- What happens when the eval store's filesystem location does not exist or is not writable?
- How does the hash chain handle an evaluation run with zero invocations?
- What happens when two suite runs have the same identifier?
- How does the runner handle concurrent execution of multiple suites?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The system MUST orchestrate evaluation suite execution, running each case against the agent and collecting results.
- **FR-002**: The system MUST load eval case definitions from a structured data file format.
- **FR-003**: The system MUST continue executing remaining cases when one case fails.
- **FR-004**: The system MUST provide an evaluator abstraction that can be implemented for custom evaluation strategies.
- **FR-005**: The system MUST provide a registry for composing multiple evaluators, with built-in defaults pre-registered.
- **FR-006**: The system MUST aggregate scores across cases (mean, min, max) and across evaluators.
- **FR-007**: The system MUST evaluate suite results against configurable gate thresholds (pass rate, cost, duration) and produce a pass/fail decision.
- **FR-008**: The gate MUST report all threshold violations, not just the first.
- **FR-009**: The system MUST persist evaluation results to the filesystem in a structured, human-readable format.
- **FR-010**: The system MUST support listing and loading historical evaluation runs.
- **FR-011**: The system MUST produce tamper-evident audit trails using cryptographic hash chains.
- **FR-012**: The system MUST support integrity verification of audit trails, detecting and localizing tampering.
- **FR-013**: The system MUST provide a budget evaluator that scores runs based on token usage, cost, and duration relative to configured limits.

### Key Entities

- **EvalRunner**: The orchestrator that executes an evaluation suite, applying evaluators to each case and producing aggregate results.
- **EvalCase**: A single test case definition containing a prompt, expected behavior, and metadata. Loaded from a structured data file.
- **Evaluator**: The abstraction for a scoring strategy. Receives a trajectory and produces a score. Custom implementations can be registered.
- **EvaluatorRegistry**: A collection of evaluators applied to each case. Pre-populated with built-in defaults and extensible with custom evaluators.
- **Score**: A numeric result from an evaluator, with a name, value (0.0-1.0), and optional details. Aggregated across cases and evaluators.
- **GateConfig**: Threshold configuration for deployment gating: minimum pass rate, maximum cost, maximum duration.
- **EvalStore**: The abstraction for persisting and retrieving evaluation results. The filesystem implementation stores results as structured data files.
- **AuditedInvocation**: A tool invocation record augmented with a cryptographic hash chaining to the previous record, forming a tamper-evident log.
- **BudgetEvaluator**: An evaluator that scores resource consumption (tokens, cost, duration) against configured budget limits.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A multi-case evaluation suite runs to completion and produces per-case and aggregate scores.
- **SC-002**: Gate decisions are deterministic — the same results and thresholds always produce the same pass/fail outcome.
- **SC-003**: Evaluation results survive process restarts — saved results can be loaded and compared across runs.
- **SC-004**: Tampered audit trails are detected with 100% reliability when verified.
- **SC-005**: Custom evaluators integrate into the pipeline identically to built-in evaluators — no special handling required.
- **SC-006**: Suite execution is resilient — a failing case does not prevent the remaining cases from running.

## Assumptions

- The trajectory collection and matching capabilities from spec 023 are available for use by evaluators.
- Eval case data files use a widely-supported structured format that supports nested data and lists.
- The cryptographic hash function used for audit trails is a standard, well-known algorithm (not a custom hash).
- The eval runner executes cases sequentially by default; parallel execution is a future enhancement.
- Score values are normalized to the range 0.0-1.0 for consistency across evaluators.
- The filesystem-based eval store is sufficient for local development and CI; remote storage backends are a future extension.
