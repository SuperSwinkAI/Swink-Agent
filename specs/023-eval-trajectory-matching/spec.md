# Feature Specification: Eval: Trajectory & Matching

**Feature Branch**: `023-eval-trajectory-matching`
**Created**: 2026-03-20
**Status**: Scope expanded 2026-04-21 — semantic matching & env-state assertions added (US5–US7, FR-010–FR-013)
**Input**: Trajectory collection from agent event stream, golden-path comparison, efficiency scoring, response matching. TrajectoryCollector (AgentEvent to Invocation traces), TrajectoryMatcher (golden path comparison), EfficiencyEvaluator (duplicate ratio 0.6 weight + step ratio 0.4 weight), ResponseCriteria (exact, contains, regex, custom closure). References: HLD Evaluation Layer, EVAL.md (Trajectory & Process, Advanced Verification).

## Clarifications

### Session 2026-03-23

- Q: Should 023 formally acknowledge BudgetGuard integration with TrajectoryCollector? → A: Yes — add BudgetGuard as a key entity since it's integral to trajectory collection. **[Superseded 2026-04-21: BudgetGuard is fully removed from 023. Budget enforcement is delegated to `BudgetPolicy` (PreTurn slot) and `MaxTurnsPolicy` from `swink-agent-policies`, attached to the agent by the `AgentFactory`. Mid-turn cancellation and wall-clock deadline (`max_duration`) capabilities from the old guard are dropped — accepted losses. See Phase 13 migration tasks.]**
- Q: Should the spec document all three TrajectoryMatcher modes (Exact, InOrder, AnyOrder)? → A: Yes — document all three with InOrder as default.
- Q: Should spec entity names align with the implementation's hierarchy (Invocation → TurnRecord → RecordedToolCall)? → A: Yes — update to match code structure.
- Q: Should the spec resolve all 8 edge cases with answers from the implementation? → A: Yes — resolve all 8.
- Q: Should the spec efficiency formula align with the implementation's exact formula? → A: Yes — update FR-005 to match code.
- Q: Is ResponseCriteria a single criterion per EvalCase or a composite? → A: Single criterion per case; composites use Custom closures.

### Session 2026-04-21 (Scope expansion — semantic matching & env-state)

- Q: Should 023 add semantic tool-selection / parameter-intent matching and environment-state assertions? → A: Yes — both fit the "Trajectory & Matching" theme. The shared LLM-judge infrastructure (concrete model client implementations, prompt-template registry, retry/backoff policy, etc.) is forward-referenced from spec 043 (`043-evals-adv-features`).
- Q: Where does the `JudgeClient` trait live? → A: Defined in `swink-agent-eval` (this crate) as a minimal async interface. Concrete implementations live in spec 043 — 023 ships only the trait + the evaluators that consume it.
- Q: How should semantic evaluators behave when no `JudgeClient` is configured or no semantic criterion is set on the case? → A: Return `None` (not applicable) — same convention as every other built-in evaluator. Eval runs remain usable without judge infra.
- Q: How is environment state captured for assertion? → A: A `StateCapture` callback (`Arc<dyn Fn(&Invocation) -> Vec<EnvironmentState> + Send + Sync>`) is registered on the `EvalCase` (or supplied by the `AgentFactory`). Trajectory collection stays free of tool- or domain-specific logic.
- Q: How are JudgeClient failures (network errors, malformed responses, timeouts) and state-capture-callback panics handled? → A: Same panic-isolation contract as the rest of the registry — convert to `Score::fail()` with diagnostic context; never propagate.
- Q: How is the non-hang guarantee for semantic evaluators enforced when the `JudgeClient` trait has no deadline parameter? → A: Semantic evaluators (US5, US6) wrap each judge call in an evaluator-side `tokio::time::timeout` with a configurable maximum (default 5 minutes, set via `with_timeout(Duration)`). An outer timeout elapse maps to `Score::fail()` with the same treatment as `JudgeError::Timeout` from the implementation. This gives 023 its own non-hang guarantee independent of the concrete `JudgeClient` impl (which lives in spec 043).
- Q: Should `BudgetGuard` remain as the collector-level safety net, or be fully replaced by 031's policies? → A: Full port. `BudgetGuard` is deleted; budget enforcement moves to the agent loop via `BudgetPolicy` (PreTurn slot) and `MaxTurnsPolicy` from `swink-agent-policies`. The `AgentFactory` is responsible for attaching these policies based on `EvalCase.budget`. Mid-turn cancellation and wall-clock `max_duration` are dropped (accepted losses — see Phase 13 migration tasks).

### User Story 1 - Capture Execution Traces from Agent Runs (Priority: P1)

An evaluator runs an agent against a test case and wants to understand exactly what happened: which tools were called, in what order, with what inputs, and what results they produced. The trajectory collector observes the agent event stream and builds a structured trace of every invocation. After the run completes, the evaluator inspects the trace to understand the agent's behavior step by step.

**Why this priority**: Trajectory collection is the foundation for all evaluation — without traces, nothing can be compared, scored, or analyzed.

**Independent Test**: Can be tested by running an agent with known tool calls, collecting the trajectory, and verifying the trace contains every invocation in the correct order with correct inputs and outputs.

**Acceptance Scenarios**:

1. **Given** an agent run that invokes multiple tools, **When** the trajectory collector observes the event stream, **Then** each tool invocation is captured with its name, inputs, and result.
2. **Given** an agent run with multiple turns, **When** collection completes, **Then** invocations are ordered chronologically across all turns.
3. **Given** an agent run where a tool call fails, **When** the trajectory is collected, **Then** the failure is captured as part of the invocation record (not silently dropped).
4. **Given** an agent run with no tool calls (text-only response), **When** the trajectory is collected, **Then** the trace contains zero invocations but records the response.

---

### User Story 2 - Compare Execution Against a Golden Path (Priority: P1)

An evaluator defines a golden path — the expected sequence of tool invocations for a given test case. After the agent run completes, the trajectory matcher compares the actual trace against the golden path. The evaluator sees which expected steps were executed, which were missed, and which unexpected steps occurred. This tells the evaluator whether the agent followed the intended approach.

**Why this priority**: Golden-path comparison is the primary mechanism for verifying that the agent solves problems correctly — it validates process, not just output.

**Independent Test**: Can be tested by defining a golden path with three expected steps, running an agent that executes two of them plus one extra, and verifying the matcher reports one match, one miss, and one unexpected step.

**Acceptance Scenarios**:

1. **Given** a golden path and an actual trajectory that matches exactly, **When** compared, **Then** all steps are reported as matched.
2. **Given** a golden path and an actual trajectory with missing steps, **When** compared, **Then** the missing steps are clearly identified.
3. **Given** a golden path and an actual trajectory with extra steps not in the golden path, **When** compared, **Then** the extra steps are identified as unexpected.
4. **Given** a golden path and an actual trajectory with steps in a different order, **When** compared, **Then** the ordering deviation is reported.

---

### User Story 3 - Score Agent Efficiency (Priority: P2)

An evaluator wants to measure how efficiently the agent solved a problem — did it take unnecessary steps or repeat the same tool calls? The efficiency evaluator computes a score based on two factors: the ratio of unique to total tool calls (weighted at 0.6) and the ratio of ideal to actual turns (weighted at 0.4). Ideal turns are derived from `budget.max_turns` if set, otherwise `unique_call_count.max(1)`. A perfect score means no duplicates and exactly the ideal number of turns. The evaluator uses this score to compare different models or prompts on the same task.

**Why this priority**: Efficiency scoring enables quantitative comparison between agent configurations, but the binary pass/fail of golden-path matching is more immediately useful.

**Independent Test**: Can be tested by providing trajectories with known duplicate counts and step counts, computing the efficiency score, and verifying it matches the expected weighted calculation.

**Acceptance Scenarios**:

1. **Given** a trajectory with no duplicates and exactly the expected number of steps, **When** scored, **Then** the efficiency score is 1.0 (perfect).
2. **Given** a trajectory with 50% duplicate invocations and twice the expected steps, **When** scored, **Then** the score reflects both penalties according to the 0.6/0.4 weighting.
3. **Given** a trajectory with zero steps (empty), **When** scored, **Then** the efficiency score is 0.0.
4. **Given** two trajectories for the same task, **When** both are scored, **Then** the more efficient trajectory receives a higher score.

---

### User Story 4 - Match Response Content Against Criteria (Priority: P2)

An evaluator wants to verify that the agent's final response meets specific content criteria. They define response criteria using one of several matching strategies: the response must exactly equal an expected string, must contain a substring, must match a regular expression pattern, or must satisfy a custom matching function. The evaluator combines multiple criteria to build comprehensive response checks.

**Why this priority**: Response matching complements trajectory matching — trajectory verifies process, response matching verifies output. Together they provide full coverage.

**Independent Test**: Can be tested by providing responses and criteria of each type and verifying that matches and mismatches are correctly reported.

**Acceptance Scenarios**:

1. **Given** an exact-match criterion, **When** the response matches exactly, **Then** the criterion passes; when it differs, the criterion fails.
2. **Given** a contains criterion, **When** the response contains the expected substring, **Then** the criterion passes.
3. **Given** a regex criterion, **When** the response matches the pattern, **Then** the criterion passes.
4. **Given** a custom matching function, **When** the response satisfies it, **Then** the criterion passes.
5. **Given** a Custom criterion that internally combines multiple checks, **When** all sub-checks pass, **Then** the criterion passes; when any sub-check fails, the criterion fails with details of which sub-check failed.

---

### User Story 5 - Score Tool Selection Semantically (Priority: P2)

An evaluator wants to verify that the agent picked the *right tool* for a given step, beyond literal name equality. When multiple tools could plausibly solve the problem (e.g. `read_file` vs `fetch_document`), a semantic judge inspects the user goal, available tools, session history, and the chosen tool, and scores whether the selection was appropriate.

**Why this priority**: Deterministic name-matching from US2 misses cases where a semantically equivalent tool is called. P2 because literal matching covers the majority of test cases and the semantic judge introduces an external LLM dependency.

**Independent Test**: Provide a case where the golden path expects `read_file` but the agent calls `fetch_document`; deterministic matcher reports a miss, semantic matcher accepts it when the configured `JudgeClient` deems them equivalent.

**Acceptance Scenarios**:

1. **Given** a case with an injected `JudgeClient` and a tool-selection criterion, **When** the agent calls a semantically equivalent tool, **Then** the evaluator returns Pass with the judge's reason in details.
2. **Given** a case with no `JudgeClient` configured, **When** the semantic tool-selection evaluator runs, **Then** it returns `None` (not applicable) — never panics, never hard-fails.
3. **Given** a case with no semantic tool-selection criterion set, **When** the evaluator runs, **Then** it returns `None`.
4. **Given** a malformed judge response (unparseable or schema violation), **When** the evaluator processes it, **Then** the evaluator returns `Score::fail()` with details of the parse error.
5. **Given** a `JudgeClient` that returns a transport/network error, **When** the evaluator handles it, **Then** the evaluator returns `Score::fail()` with the error and the rest of the eval set continues.

---

### User Story 6 - Score Tool Parameters Semantically (Priority: P2)

An evaluator wants to verify the agent invoked a tool with the *right intent* in its arguments — even when the exact JSON differs. A semantic judge compares an expected parameter-intent string (e.g. "read the config file for project alpha") against the actual arguments JSON.

**Why this priority**: Exact JSON equality from US2 is brittle — equivalent paths, different key orderings, paraphrased prompts, or unrelated extra fields all fail. P2 for the same reasons as US5.

**Independent Test**: Provide a case with `expected_tool_intent: "read config for project-alpha"`; the agent calls a tool with `{"path": "./project-alpha/config.toml"}`. Semantic matcher accepts; deterministic matcher (US2) would reject.

**Acceptance Scenarios**:

1. **Given** a case with `expected_tool_intent` and an injected `JudgeClient`, **When** the actual arguments satisfy the intent semantically, **Then** the evaluator returns Pass with judge reason in details.
2. **Given** a case with no `expected_tool_intent` set, **When** the evaluator runs, **Then** it returns `None`.
3. **Given** a `JudgeClient` that times out, **When** the evaluator handles the timeout, **Then** it returns `Score::fail()` with timeout context — no runaway hang, no panic.
4. **Given** an `expected_tool_intent` paired with a specific `tool_name` filter, **When** the agent calls a different tool, **Then** the evaluator skips that call (not Pass, not Fail — only the targeted tool's params are scored).

---

### User Story 7 - Verify Environment-State Assertions (Priority: P2)

An evaluator wants to assert that the agent's actions produced the expected side effects on the environment — files written, database rows updated, counters incremented. A `StateCapture` callback runs after the agent finishes; named environment states are compared deterministically against `expected_environment_state` on the case.

**Why this priority**: Many agent tasks are defined by what they *do*, not what they *say*. Trajectory matching catches a subset of these cases; explicit state assertions catch the rest. P2 because the existing trajectory + response matchers already validate most behavior.

**Independent Test**: Provide a case with `expected_environment_state: [{name: "created_file", state: "out.md"}]` plus a state-capture callback that lists the working directory. Run an agent that writes `out.md`. Evaluator returns Pass.

**Acceptance Scenarios**:

1. **Given** a case with `expected_environment_state` and a `StateCapture` callback, **When** the actual captured state equals expected for every named entry, **Then** the evaluator returns Pass with the matched state names in details.
2. **Given** a named state in `expected_environment_state` that is missing from the captured snapshot, **When** evaluated, **Then** the evaluator returns Fail identifying the missing state by name.
3. **Given** a named state whose captured value differs from the expected value, **When** evaluated, **Then** the evaluator returns Fail with both expected and actual values in details.
4. **Given** a case with `expected_environment_state` set but no `StateCapture` callback registered, **When** the evaluator runs, **Then** it returns `None` (capture not configured) and the eval set continues.
5. **Given** a `StateCapture` callback that panics, **When** the evaluator runs, **Then** it returns `Score::fail()` with the panic message in details — never propagates the panic.

---

### Edge Cases

- **Empty golden path (no expected steps)**: Behavior varies by match mode. Exact: returns 0.0 if actual has any steps (length mismatch), 0.0 if both empty (0/1 matched). InOrder/AnyOrder: returns pass (vacuous truth — all zero expected calls were found). This follows standard evaluation framework semantics.
- **Identical tool names, different inputs**: Matching is by tool name only by default. `ExpectedToolCall.arguments` is optional (`Option<Value>`); when `Some`, both name and arguments must match. When `None`, name-only matching applies.
- **Extremely long trajectory (thousands of invocations)**: No artificial limit. Collection and matching operate over `Vec` structures in memory; performance scales linearly with invocation count.
- **Efficiency scoring with zero expected steps**: Ideal turns falls back to `unique_call_count.max(1)`. If total tool calls is also 0, the evaluator returns `None` (not applicable) rather than producing a degenerate score.
- **Invalid regular expression in regex criterion**: Criterion fails with a diagnostic error message containing the regex compilation error. Does not panic.
- **Concurrent tool executions completing out of order**: Captured in the order their `ToolExecutionStart` events arrive in the stream. The trajectory reflects event-stream ordering, not wall-clock completion order.
- **Custom matching function panics**: Treated as a criterion failure with diagnostic context describing the panic. Does not propagate the panic to the caller.
- **Partial matches (correct tool name, wrong input)**: When `ExpectedToolCall.arguments` is `None`, the step matches (name-only). When `arguments` is `Some` and doesn't match, the step is not matched — reported as missing (expected) and unexpected (actual).
- **JudgeClient connection failure**: Semantic evaluators (US5, US6) return `Score::fail()` with network/transport error details. The case continues to other evaluators; the eval set continues to subsequent cases.
- **JudgeClient malformed response**: Semantic evaluators return `Score::fail()` with the parse error and a snippet of the offending response (truncated to a safe length). No panic.
- **JudgeClient timeout**: Semantic evaluators return `Score::fail()` with the timeout context. Two timeout sources exist and are both mapped identically: (a) `JudgeError::Timeout` returned by the concrete `JudgeClient` implementation's own deadline handling, and (b) the evaluator's outer `tokio::time::timeout` elapsing past the configured maximum (default 5 minutes). Neither blocks the runner.
- **State capture callback panics**: Caught via `catch_unwind`; `EnvironmentStateEvaluator` returns `Score::fail()` with the panic message in details.
- **Multiple expected environment states with the same name**: Rejected at case load time as a validation error — no first-wins, no last-wins, no silent dedup.
- **Captured environment state contains entries not declared in `expected_environment_state`**: Ignored. Only declared expected names are scored. Extras are not failures.
- **Semantic evaluator runs against an empty trajectory**: Returns `None` — no tool calls means no semantic tool selection or parameter judgments to make.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The system MUST collect structured invocation traces from the agent event stream, capturing tool name, inputs, result, and success/failure status for each invocation.
- **FR-002**: The system MUST preserve chronological ordering of invocations across multiple turns.
- **FR-003**: The system MUST compare an actual trajectory against a golden path, identifying matched, missing, and unexpected steps. Three match modes MUST be supported: Exact (strict sequence, no extras), InOrder (ordered subsequence, extras allowed — default), and AnyOrder (set membership, order irrelevant).
- **FR-004**: The system MUST report ordering deviations between the actual trajectory and the golden path (applicable to Exact and InOrder modes).
- **FR-005**: The system MUST compute an efficiency score using the formula: `(unique_calls / total_calls) * 0.6 + (min(ideal_turns, actual_turns) / actual_turns) * 0.4`, clamped to [0.0, 1.0]. `ideal_turns` is derived from `budget.max_turns` if set, otherwise `unique_call_count.max(1)`. Returns `None` (not applicable) when total tool calls is 0.
- **FR-006**: The system MUST support four response matching strategies: exact equality, substring containment, regular expression, and custom matching function.
- **FR-007**: Each EvalCase has a single `ResponseCriteria` (one of: Exact, Contains, Regex, Custom). To combine multiple checks, use a Custom closure that internally evaluates multiple conditions and reports which sub-checks failed.
- **FR-008**: The system MUST handle custom matching function failures (panics or errors) gracefully, treating them as criterion failures with diagnostic context.
- **FR-009**: The system MUST capture failed tool invocations in the trajectory, not silently drop them.
- **FR-010**: The system MUST expose a `JudgeClient` trait (single async method returning a structured `JudgeVerdict`) that semantic evaluators consume. Concrete `JudgeClient` implementations are out of scope for this spec and are provided by spec 043 (`043-evals-adv-features`). Semantic evaluators (FR-011, FR-012) MUST wrap each judge call in an evaluator-side `tokio::time::timeout` with a configurable maximum (default 5 minutes, exposed via `with_timeout(Duration)`). Both an inner `JudgeError::Timeout` (from the impl) and an outer timeout elapse map to `Score::fail()` with timeout context (see FR-014).
- **FR-011**: The system MUST support semantic tool-selection scoring — for each actual tool call, ask a configured `JudgeClient` whether the chosen tool was appropriate given the user goal, available tools, and session history. Returns `None` when no `JudgeClient` is configured or no semantic tool-selection criterion is set on the case.
- **FR-012**: The system MUST support semantic tool-parameter scoring — for each actual tool call (optionally filtered to a specific tool name), compare an expected parameter-intent string against the actual JSON arguments via the `JudgeClient`. Returns `None` when the criterion is not set on the case.
- **FR-013**: The system MUST support environment-state assertions — a `StateCapture` callback (registered on the `EvalCase`) produces a `Vec<EnvironmentState>` after the agent completes; an `EnvironmentStateEvaluator` compares each named state against `expected_environment_state` deterministically. Missing names are reported as failures; values are compared for JSON equality. Returns `None` when no capture callback is configured for the case.
- **FR-014**: The system MUST handle `JudgeClient` failures (transport errors, malformed responses, inner deadline timeouts, and outer evaluator-side `tokio::time::timeout` elapses) and `StateCapture` callback panics by returning `Score::fail()` with diagnostic context — never propagating to the runner. This extends the per-evaluator panic-isolation contract from FR-008 (custom response closures) to judge clients and state-capture callbacks.
- **FR-015**: The system MUST validate `expected_environment_state` at case load time, rejecting duplicate state names with a clear error (no silent dedup, no first/last-wins).

### Key Entities

- **TrajectoryCollector**: Observes the agent event stream and builds a structured Invocation trace, organized by turns. Supports both incremental observation (`observe()`) and stream consumption (`collect_from_stream()`, `collect_with_guard()`).
- **Invocation**: The top-level trace of an entire agent run, containing a `Vec<TurnRecord>`, total usage, total cost, total duration, final response text, stop reason, and model info.
- **TurnRecord**: A single turn within an Invocation, containing the turn index, assistant message, tool calls (`Vec<RecordedToolCall>`), tool results, and turn duration.
- **RecordedToolCall**: A single tool call record within a turn, containing the tool call id, tool name, and arguments.
- **TrajectoryMatcher**: Compares an actual trajectory against a golden path (expected invocation sequence), producing a comparison report of matched, missing, and unexpected steps. Supports three match modes: **Exact** (strict sequence, no extras allowed), **InOrder** (golden steps must appear in order, extras allowed between them — the default), and **AnyOrder** (all golden steps must appear, order irrelevant).
- **EfficiencyEvaluator**: Scores a trajectory's efficiency based on duplicate invocation ratio (0.6 weight) and step count ratio (0.4 weight).
- **ResponseCriteria**: A set of matching rules applied to the agent's final response, supporting exact, contains, regex, and custom matching strategies.
- **BudgetPolicy / MaxTurnsPolicy** (from `swink-agent-policies`): Budget enforcement at the agent loop's pre-turn boundary. `BudgetPolicy` caps `max_cost`, `max_input`, `max_output`; `MaxTurnsPolicy` caps turn count. The `AgentFactory` reads `EvalCase.budget` and attaches the derived policies to the agent via `AgentOptions::with_pre_turn_policy(...)`. Enforcement fires at turn boundaries only — mid-turn cancellation and wall-clock deadline (`max_duration`) are not supported in 023 (accepted losses from the BudgetGuard → BudgetPolicy migration, Phase 13).
- **JudgeClient** (trait): Minimal async interface for LLM-based judging consumed by semantic evaluators. Single method shape: `async fn judge(&self, prompt: &str) -> Result<JudgeVerdict, JudgeError>`. Concrete implementations (model providers, prompt templating, retry/backoff, batching) are out of scope for 023 and live in spec 043 (`043-evals-adv-features`). `swink-agent-eval` exposes only the trait so 023's semantic evaluators compile and unit-test against test doubles. The trait has no deadline parameter; the non-hang guarantee is enforced evaluator-side (see `SemanticToolSelectionEvaluator` / `SemanticToolParameterEvaluator` entities and FR-010).
- **JudgeVerdict**: Structured output from a `JudgeClient`. Shape: `{ score: f64 in [0.0, 1.0], pass: bool, reason: Option<String>, label: Option<String> }`. Mirrors strands-evals' `EvaluationOutput` so future provider implementations can map cleanly.
- **JudgeError**: Error type returned by `JudgeClient`. Variants: `Transport`, `Timeout`, `MalformedResponse`, `Other`. Semantic evaluators map any variant to `Score::fail()` with the variant in details (FR-014).
- **SemanticToolSelectionEvaluator**: For each actual tool call in the invocation, asks a configured `JudgeClient` whether the chosen tool was appropriate given the user goal, available tools (from the agent's tool set), and session history up to that point. Returns `None` when no judge is configured or no semantic criterion is set. Returns Pass/Fail aggregated across all judged calls. Each judge call is wrapped in `tokio::time::timeout` with a configurable maximum — `with_timeout(Duration)` override, default 5 minutes. Outer timeout elapses map to `Score::fail()` (FR-010, FR-014).
- **SemanticToolParameterEvaluator**: For each actual tool call (optionally filtered to a target tool name), asks a configured `JudgeClient` whether the JSON arguments satisfy a declared `expected_tool_intent` string from the case. Returns `None` when no intent is set. Returns Pass/Fail with judge reasons in details. Judge calls are bounded by the same configurable evaluator-side timeout as `SemanticToolSelectionEvaluator` (default 5 minutes).
- **EnvironmentStateEvaluator**: Deterministic evaluator that compares a captured environment-state snapshot against `expected_environment_state` on the case. No LLM dependency. Returns Pass when every expected named state matches actual; Fail otherwise.
- **EnvironmentState** (data): Named state entry — `{ name: String, state: serde_json::Value }`. Compared for full JSON equality (consistent with `ExpectedToolCall.arguments`).
- **StateCapture** (callback): `Arc<dyn Fn(&Invocation) -> Vec<EnvironmentState> + Send + Sync>`. Registered on the `EvalCase` (or supplied by the `AgentFactory`). Invoked once after the agent completes; output populates the "actual" side for `EnvironmentStateEvaluator`. Panics are caught and surfaced as `Score::fail()` (FR-014).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Every tool invocation in an agent run appears in the collected trajectory — zero invocations are silently dropped.
- **SC-002**: Golden-path comparison correctly identifies all matched, missing, and unexpected steps for any pair of trajectory and golden path.
- **SC-003**: Efficiency scores are deterministic — the same trajectory and golden path always produce the same score.
- **SC-004**: All four response matching strategies (exact, contains, regex, custom) correctly distinguish matching from non-matching responses.
- **SC-005**: The efficiency formula produces 1.0 for a perfect trajectory and 0.0 for an empty trajectory.
- **SC-006**: Semantic evaluators (US5, US6) with no configured `JudgeClient` or no semantic criterion set return `None` and never cause the case or set to fail.
- **SC-007**: Environment-state assertions correctly identify all matching, missing, and value-mismatched named states for any pair of expected and captured snapshots.
- **SC-008**: Every external call from US5–US7 evaluators (judge client request, state capture callback) is panic-isolated — a failure or panic in one evaluator never aborts the case, the registry, or the eval run.
- **SC-009**: A duplicate name in `expected_environment_state` is rejected at case load with a clear validation error pointing to the offending name.

## Assumptions

- The agent event stream provides sufficient information to reconstruct tool invocations (tool name, inputs, outputs, timing).
- Golden paths are defined ahead of time by the evaluator as part of test case setup.
- The efficiency scoring weights (0.6 for duplicates, 0.4 for step count) are fixed in this specification; configurable weights are a future enhancement.
- Response matching operates on the final assistant text response, not intermediate streaming tokens.
- The trajectory collector is a passive observer — it does not modify the agent's behavior.
- Concrete `JudgeClient` implementations (model providers, prompt templates, retry/backoff, caching) are out of scope for 023 and are delivered by spec 043 (`043-evals-adv-features`). 023 ships only the trait surface, an evaluator-side outer timeout (default 5 min), and the evaluators that depend on the trait.
- Semantic evaluators (US5, US6) are tested in 023 against in-process `JudgeClient` test doubles (fakes returning canned `JudgeVerdict`s, plus a slow fake that exceeds the evaluator's outer timeout). Live LLM judging is exercised in spec 043.
- Environment-state capture is application-specific. 023 defines the callback contract and the comparison evaluator; each consumer crate (or test) supplies its own capture closure.
- `expected_tool_intent` strings are short natural-language descriptions, not structured schemas. Spec 043 may layer a richer intent DSL on top; 023 keeps the surface area minimal.
- Budget enforcement is delegated to `swink-agent-policies`: the `AgentFactory` reads `EvalCase.budget` and attaches `BudgetPolicy` + `MaxTurnsPolicy` to the agent. The runner does not enforce budgets directly. Callers who need wall-clock deadlines or mid-turn cancellation must compose their own cancellation (outside 023's surface).
