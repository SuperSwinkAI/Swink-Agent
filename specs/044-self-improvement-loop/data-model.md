# Data Model: Self-Improvement Loop

## Types Defined in This Crate

### OptimizationTarget (struct)

The mutable agent configuration being optimized.

| Field | Type | Description |
|-------|------|-------------|
| `system_prompt` | `String` | The full system prompt text |
| `sections` | `Vec<PromptSection>` | Named sections parsed from the prompt (empty if unstructured) |
| `tool_schemas` | `Vec<ToolSchema>` | Tool definitions available to the agent |
| `section_delimiter` | `Option<Regex>` | Custom section boundary pattern (default: markdown headers) |

Constructors: `new(system_prompt, tool_schemas)`, `with_section_delimiter(regex)`.
Sections are auto-parsed on construction. Unstructured prompts produce a single unnamed section.

### PromptSection (struct)

| Field | Type | Description |
|-------|------|-------------|
| `name` | `Option<String>` | Section heading (None for unnamed/single-section prompts) |
| `content` | `String` | Section body text |
| `byte_range` | `Range<usize>` | Position within the full system prompt |

### OptimizationConfig (struct)

Configuration for an optimization cycle.

| Field | Type | Description |
|-------|------|-------------|
| `eval_set` | `EvalSet` | The eval cases to run |
| `strategies` | `Vec<Box<dyn MutationStrategy>>` | Mutation strategies to apply |
| `acceptance_threshold` | `f64` | Minimum aggregate score improvement to accept (default: 0.01) |
| `budget` | `CycleBudget` | Cost budget for the entire cycle |
| `parallelism` | `usize` | Max concurrent candidate evaluations (default: 1) |
| `seed` | `Option<u64>` | Deterministic seed for template/ablation strategies |
| `max_weak_points` | `usize` | Max weak points to process per cycle (default: 5) |
| `max_candidates_per_strategy` | `usize` | Max candidates per strategy per weak point (default: 3) |
| `output_root` | `PathBuf` | Root directory for persisted results |

Constructors: builder pattern via `new(eval_set, output_root)` + `with_*()` methods.

### CycleBudget (struct)

Shared cost accumulator across all phases.

| Field | Type | Description |
|-------|------|-------------|
| `max_cost` | `Cost` | Maximum total cost allowed |
| `spent` | `Cost` | Accumulated cost so far (interior mutability) |

Methods: `new(max_cost)`, `record(cost)`, `remaining() -> Cost`, `is_exhausted() -> bool`.

### BaselineSnapshot (struct)

Results of evaluating the original configuration.

| Field | Type | Description |
|-------|------|-------------|
| `target` | `OptimizationTarget` | The original configuration |
| `results` | `Vec<EvalCaseResult>` | Per-case results from the eval run |
| `aggregate_score` | `f64` | Arithmetic mean of case scores |
| `cost` | `Cost` | Cost of the baseline evaluation |

### WeakPoint (struct)

A diagnosed improvement opportunity.

| Field | Type | Description |
|-------|------|-------------|
| `component` | `TargetComponent` | Which part of the config to improve |
| `affected_cases` | `Vec<CaseFailure>` | Eval cases that failed on this component |
| `mean_score_gap` | `f64` | Average (threshold - actual) across affected cases |
| `severity` | `f64` | `affected_cases.len() * mean_score_gap` |

### TargetComponent (enum)

| Variant | Fields | Description |
|---------|--------|-------------|
| `PromptSection` | `{ index: usize, name: Option<String> }` | A named section of the system prompt |
| `ToolDescription` | `{ tool_name: String }` | A specific tool's description/schema |
| `FullPrompt` | — | The entire system prompt (when no sections parsed) |

### CaseFailure (struct)

| Field | Type | Description |
|-------|------|-------------|
| `case_id` | `String` | The eval case identifier |
| `evaluator_name` | `String` | Which evaluator reported the failure |
| `score` | `Score` | The actual score achieved |
| `details` | `Option<String>` | Evaluator-provided failure details |

### Candidate (struct)

A mutated configuration variant.

| Field | Type | Description |
|-------|------|-------------|
| `id` | `String` | Unique identifier (SHA-256 hash of mutated value) |
| `component` | `TargetComponent` | Which component was mutated |
| `original_value` | `String` | The original text |
| `mutated_value` | `String` | The proposed replacement |
| `strategy` | `String` | Name of the strategy that produced it |

### CandidateResult (struct)

Evaluation results for a single candidate.

| Field | Type | Description |
|-------|------|-------------|
| `candidate` | `Candidate` | The evaluated candidate |
| `results` | `Vec<EvalCaseResult>` | Per-case eval results |
| `aggregate_score` | `f64` | Arithmetic mean of case scores |
| `cost` | `Cost` | Cost of evaluating this candidate |

### AcceptanceVerdict (enum)

| Variant | Fields | Description |
|---------|--------|-------------|
| `Accepted` | — | Meets threshold, no P1 regressions |
| `AcceptedNotApplied` | — | Accepted but outranked by a better candidate for the same component |
| `BelowThreshold` | `{ improvement: f64, threshold: f64 }` | Score improvement too small |
| `P1Regression` | `{ case_id: String }` | Regressed a P1 eval case |
| `NoImprovement` | — | Score equal or worse than baseline |

### AcceptanceResult (struct)

| Field | Type | Description |
|-------|------|-------------|
| `applied` | `Vec<(Candidate, CandidateResult)>` | Top-ranked accepted candidates (one per component) |
| `accepted_not_applied` | `Vec<(Candidate, CandidateResult)>` | Accepted but outranked |
| `rejected` | `Vec<(Candidate, CandidateResult, AcceptanceVerdict)>` | Rejected with reasons |

### CycleResult (struct)

| Field | Type | Description |
|-------|------|-------------|
| `cycle_number` | `u32` | Cycle sequence number |
| `baseline` | `BaselineSnapshot` | Starting point for this cycle |
| `weak_points` | `Vec<WeakPoint>` | Diagnosed improvement opportunities |
| `candidates_evaluated` | `usize` | Total candidates scored |
| `acceptance` | `AcceptanceResult` | Gate results |
| `total_cost` | `Cost` | Aggregate cost across all phases |
| `status` | `CycleStatus` | Completion status |
| `output_dir` | `Option<PathBuf>` | Path to persisted results (None if nothing accepted) |

### CycleStatus (enum)

| Variant | Description |
|---------|-------------|
| `Complete` | All phases ran to completion |
| `BudgetExhausted { phase: String }` | Budget ran out during the named phase |
| `NoImprovements` | All candidates rejected |
| `NoDiagnosis` | No weak points found (baseline is already strong) |

### ManifestEntry (struct)

One line in the JSONL audit manifest.

| Field | Type | Description |
|-------|------|-------------|
| `cycle_id` | `u32` | Cycle number |
| `timestamp` | `String` | ISO 8601 timestamp |
| `target_component` | `String` | Serialized `TargetComponent` |
| `original_value` | `String` | Before mutation |
| `mutated_value` | `String` | After mutation |
| `strategy` | `String` | Mutation strategy name |
| `baseline_score` | `f64` | Aggregate baseline score |
| `candidate_score` | `f64` | Aggregate candidate score |
| `verdict` | `String` | Serialized `AcceptanceVerdict` |
| `rejection_reason` | `Option<String>` | Human-readable reason if rejected |

Derives: `Serialize`, `Deserialize`.

## Traits Defined in This Crate

### MutationStrategy (trait)

```rust
pub trait MutationStrategy: Send + Sync {
    fn name(&self) -> &str;
    fn mutate(&self, target: &str, context: &MutationContext) -> Result<Vec<Candidate>, MutationError>;
}
```

### MutationContext (struct)

| Field | Type | Description |
|-------|------|-------------|
| `weak_point` | `WeakPoint` | The diagnosed issue |
| `failing_traces` | `Vec<Invocation>` | Execution traces from failing cases |
| `eval_criteria` | `String` | Description of what the eval expects |
| `seed` | `Option<u64>` | Deterministic seed |
| `max_candidates` | `usize` | Per FR-023 cap |

### MutationError (enum)

| Variant | Description |
|---------|-------------|
| `JudgeUnavailable(String)` | Judge model could not be reached |
| `BudgetExhausted` | LLM cost budget used up |
| `InvalidResponse(String)` | Judge returned unparseable response |
| `Panic(String)` | Strategy panicked (caught) |
| `Other(String)` | Catch-all |

## Types Re-used from Dependencies

From `swink-agent-eval`: `EvalRunner`, `EvalCase`, `EvalSet`, `EvalCaseResult`, `EvalSetResult`, `EvalSummary`, `Invocation`, `TurnRecord`, `Score`, `Verdict`, `EvaluatorRegistry`, `JudgeClient`, `Reporter`, `AgentFactory`, `TrajectoryCollector`.

From `swink-agent`: `ToolSchema`, `Agent`, `AgentOptions`, `Cost`, `Usage`.
