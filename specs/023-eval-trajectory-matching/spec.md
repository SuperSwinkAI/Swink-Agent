# Feature Specification: Eval: Trajectory & Matching

**Feature Branch**: `023-eval-trajectory-matching`
**Created**: 2026-03-20
**Status**: Draft
**Input**: Trajectory collection from agent event stream, golden-path comparison, efficiency scoring, response matching. TrajectoryCollector (AgentEvent to Invocation traces), TrajectoryMatcher (golden path comparison), EfficiencyEvaluator (duplicate ratio 0.6 weight + step ratio 0.4 weight), ResponseCriteria (exact, contains, regex, custom closure). References: HLD Evaluation Layer, EVAL.md (Trajectory & Process, Advanced Verification).

## Clarifications

### Session 2026-03-23

- Q: Should 023 formally acknowledge BudgetGuard integration with TrajectoryCollector? → A: Yes — add BudgetGuard as a key entity since it's integral to trajectory collection. **[Note: BudgetGuard is superseded by BudgetPolicy (PreTurn slot) per 031-policy-slots. The budget enforcement concept remains; the mechanism changes from a dedicated struct to a policy in PreTurnPolicy Slot 1.]**
- Q: Should the spec document all three TrajectoryMatcher modes (Exact, InOrder, AnyOrder)? → A: Yes — document all three with InOrder as default.
- Q: Should spec entity names align with the implementation's hierarchy (Invocation → TurnRecord → RecordedToolCall)? → A: Yes — update to match code structure.
- Q: Should the spec resolve all 8 edge cases with answers from the implementation? → A: Yes — resolve all 8.
- Q: Should the spec efficiency formula align with the implementation's exact formula? → A: Yes — update FR-005 to match code.
- Q: Is ResponseCriteria a single criterion per EvalCase or a composite? → A: Single criterion per case; composites use Custom closures.

## User Scenarios & Testing *(mandatory)*

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

### Edge Cases

- **Empty golden path (no expected steps)**: Behavior varies by match mode. Exact: returns 0.0 if actual has any steps (length mismatch), 0.0 if both empty (0/1 matched). InOrder/AnyOrder: returns pass (vacuous truth — all zero expected calls were found). This follows standard evaluation framework semantics.
- **Identical tool names, different inputs**: Matching is by tool name only by default. `ExpectedToolCall.arguments` is optional (`Option<Value>`); when `Some`, both name and arguments must match. When `None`, name-only matching applies.
- **Extremely long trajectory (thousands of invocations)**: No artificial limit. Collection and matching operate over `Vec` structures in memory; performance scales linearly with invocation count.
- **Efficiency scoring with zero expected steps**: Ideal turns falls back to `unique_call_count.max(1)`. If total tool calls is also 0, the evaluator returns `None` (not applicable) rather than producing a degenerate score.
- **Invalid regular expression in regex criterion**: Criterion fails with a diagnostic error message containing the regex compilation error. Does not panic.
- **Concurrent tool executions completing out of order**: Captured in the order their `ToolExecutionStart` events arrive in the stream. The trajectory reflects event-stream ordering, not wall-clock completion order.
- **Custom matching function panics**: Treated as a criterion failure with diagnostic context describing the panic. Does not propagate the panic to the caller.
- **Partial matches (correct tool name, wrong input)**: When `ExpectedToolCall.arguments` is `None`, the step matches (name-only). When `arguments` is `Some` and doesn't match, the step is not matched — reported as missing (expected) and unexpected (actual).

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

### Key Entities

- **TrajectoryCollector**: Observes the agent event stream and builds a structured Invocation trace, organized by turns. Supports both incremental observation (`observe()`) and stream consumption (`collect_from_stream()`, `collect_with_guard()`).
- **Invocation**: The top-level trace of an entire agent run, containing a `Vec<TurnRecord>`, total usage, total cost, total duration, final response text, stop reason, and model info.
- **TurnRecord**: A single turn within an Invocation, containing the turn index, assistant message, tool calls (`Vec<RecordedToolCall>`), tool results, and turn duration.
- **RecordedToolCall**: A single tool call record within a turn, containing the tool call id, tool name, and arguments.
- **TrajectoryMatcher**: Compares an actual trajectory against a golden path (expected invocation sequence), producing a comparison report of matched, missing, and unexpected steps. Supports three match modes: **Exact** (strict sequence, no extras allowed), **InOrder** (golden steps must appear in order, extras allowed between them — the default), and **AnyOrder** (all golden steps must appear, order irrelevant).
- **EfficiencyEvaluator**: Scores a trajectory's efficiency based on duplicate invocation ratio (0.6 weight) and step count ratio (0.4 weight).
- **ResponseCriteria**: A set of matching rules applied to the agent's final response, supporting exact, contains, regex, and custom matching strategies.
- **BudgetGuard / BudgetPolicy**: Real-time budget enforcement during trajectory collection. Monitors cost, token usage, and turn count against thresholds; cancels the agent run via CancellationToken when any limit is exceeded while allowing the trajectory collector to drain the stream and capture a complete trace. **[Note: The standalone BudgetGuard struct is superseded by BudgetPolicy in the PreTurn policy slot per 031-policy-slots. The enforcement behavior and CancellationToken integration are preserved.]**

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Every tool invocation in an agent run appears in the collected trajectory — zero invocations are silently dropped.
- **SC-002**: Golden-path comparison correctly identifies all matched, missing, and unexpected steps for any pair of trajectory and golden path.
- **SC-003**: Efficiency scores are deterministic — the same trajectory and golden path always produce the same score.
- **SC-004**: All four response matching strategies (exact, contains, regex, custom) correctly distinguish matching from non-matching responses.
- **SC-005**: The efficiency formula produces 1.0 for a perfect trajectory and 0.0 for an empty trajectory.

## Assumptions

- The agent event stream provides sufficient information to reconstruct tool invocations (tool name, inputs, outputs, timing).
- Golden paths are defined ahead of time by the evaluator as part of test case setup.
- The efficiency scoring weights (0.6 for duplicates, 0.4 for step count) are fixed in this specification; configurable weights are a future enhancement.
- Response matching operates on the final assistant text response, not intermediate streaming tokens.
- The trajectory collector is a passive observer — it does not modify the agent's behavior.
