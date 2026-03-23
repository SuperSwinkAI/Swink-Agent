# Data Model: Eval Runner, Scoring & Governance

**Feature**: 024-eval-runner-governance
**Date**: 2026-03-23

## Entities

### EvalCase

A single evaluation scenario defining what to test and how to score it.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | `String` | Yes | Unique identifier |
| `name` | `String` | Yes | Human-readable name |
| `description` | `Option<String>` | No | What this case tests |
| `system_prompt` | `String` | Yes | System prompt for the agent |
| `user_messages` | `Vec<String>` | Yes | Initial user messages (the prompt) |
| `expected_trajectory` | `Option<Vec<ExpectedToolCall>>` | No | Golden path tool calls |
| `expected_response` | `Option<ResponseCriteria>` | No | Expected final response |
| `budget` | `Option<BudgetConstraints>` | No | Resource limits |
| `evaluators` | `Vec<String>` | No | Filter: which evaluators to run (empty = all) |
| `metadata` | `serde_json::Value` | No | Arbitrary user-defined metadata |

**Identity**: `id` field (unique within an `EvalSet`)
**Serialization**: JSON (primary), YAML (opt-in via `yaml` feature)

### EvalSet

A named collection of evaluation cases.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | `String` | Yes | Unique identifier |
| `name` | `String` | Yes | Human-readable name |
| `description` | `Option<String>` | No | Purpose of this set |
| `cases` | `Vec<EvalCase>` | Yes | The cases in this set |

**Identity**: `id` field (unique within store)
**Persistence**: `{store_dir}/sets/{id}.json`

### Invocation

Complete trace of an agent run, built by `TrajectoryCollector`.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `turns` | `Vec<TurnRecord>` | Yes | All turns in execution order |
| `total_usage` | `Usage` | Yes | Aggregated token usage |
| `total_cost` | `Cost` | Yes | Aggregated cost |
| `total_duration` | `Duration` | Yes | Wall-clock duration |
| `final_response` | `Option<String>` | No | Final assistant text |
| `stop_reason` | `StopReason` | Yes | Why the agent stopped |
| `model` | `ModelSpec` | Yes | Model used |

**Lifecycle**: Created empty by `TrajectoryCollector::new()`, populated by `observe()` events, finalized by `collect_from_stream()` or `collect_with_guard()`.

### TurnRecord

A single turn within an invocation.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `turn_index` | `usize` | Yes | Zero-based index |
| `assistant_message` | `AssistantMessage` | Yes | The assistant's response |
| `tool_calls` | `Vec<RecordedToolCall>` | Yes | Tool calls made |
| `tool_results` | `Vec<ToolResultMessage>` | Yes | Tool results returned |
| `duration` | `Duration` | Yes | Wall-clock duration of this turn |

### RecordedToolCall

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | `String` | Yes | Provider-assigned tool call ID |
| `name` | `String` | Yes | Tool name |
| `arguments` | `serde_json::Value` | Yes | Parsed JSON arguments |

### ExpectedToolCall

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `tool_name` | `String` | Yes | Expected tool name |
| `arguments` | `Option<serde_json::Value>` | No | Expected arguments (exact JSON equality if present) |

### ResponseCriteria (enum)

Tagged enum (`#[serde(tag = "mode")]`) for matching the final response.

| Variant | Fields | Description |
|---------|--------|-------------|
| `Exact` | `expected: String` | Exact text match |
| `Contains` | `substring: String` | Substring search |
| `Regex` | `pattern: String` | Regex pattern match |
| `Custom` | `Arc<dyn Fn(&str) -> Score>` | Programmatic scoring (`#[serde(skip)]`) |

### BudgetConstraints

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `max_cost` | `Option<f64>` | No | Maximum cost in dollars |
| `max_tokens` | `Option<u64>` | No | Maximum total tokens |
| `max_turns` | `Option<usize>` | No | Maximum turns |
| `max_duration` | `Option<Duration>` | No | Maximum wall-clock time |

### Score

| Field | Type | Description |
|-------|------|-------------|
| `value` | `f64` | Clamped to [0.0, 1.0] |
| `threshold` | `f64` | Pass threshold (default 0.5) |

**Constructors**: `Score::pass()` (1.0/0.5), `Score::fail()` (0.0/0.5), `Score::new(value, threshold)`
**Verdict derivation**: `value >= threshold` → `Verdict::Pass`, else `Verdict::Fail`

### Verdict (enum)

| Variant | Description |
|---------|-------------|
| `Pass` | Score meets or exceeds threshold |
| `Fail` | Score below threshold |

### EvalMetricResult

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `evaluator_name` | `String` | Yes | Which evaluator produced this |
| `score` | `Score` | Yes | Numeric score |
| `details` | `Option<String>` | No | Human-readable explanation |

### EvalCaseResult

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `case_id` | `String` | Yes | Which case was evaluated |
| `invocation` | `Invocation` | Yes | Captured trajectory |
| `metric_results` | `Vec<EvalMetricResult>` | Yes | Per-evaluator results |
| `verdict` | `Verdict` | Yes | Overall case verdict (AND of all metrics) |

### EvalSetResult

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `eval_set_id` | `String` | Yes | Which set was evaluated |
| `case_results` | `Vec<EvalCaseResult>` | Yes | Per-case results |
| `summary` | `EvalSummary` | Yes | Aggregate statistics |
| `timestamp` | `u64` | Yes | Unix timestamp of run |

**Persistence**: `{store_dir}/results/{eval_set_id}/{timestamp}.json`

### EvalSummary

| Field | Type | Description |
|-------|------|-------------|
| `total_cases` | `usize` | Total cases evaluated |
| `passed` | `usize` | Cases passing all metrics |
| `failed` | `usize` | Cases failing at least one metric |
| `total_cost` | `Cost` | Aggregated cost |
| `total_usage` | `Usage` | Aggregated token usage |
| `total_duration` | `Duration` | Total wall-clock time |

### GateConfig

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `min_pass_rate` | `Option<f64>` | No | Minimum pass fraction [0.0, 1.0] |
| `max_cost` | `Option<f64>` | No | Maximum total cost |
| `max_duration` | `Option<Duration>` | No | Maximum total duration |

**Builder**: `GateConfig::new().with_min_pass_rate(0.95).with_max_cost(10.0).with_max_duration(...)`

### GateResult

| Field | Type | Description |
|-------|------|-------------|
| `passed` | `bool` | Whether gate passed |
| `exit_code` | `i32` | 0 for pass, 1 for fail |
| `summary` | `String` | Human-readable summary |

### AuditedInvocation

| Field | Type | Description |
|-------|------|-------------|
| `invocation` | `Invocation` | Original trace |
| `turn_hashes` | `Vec<String>` | Hex-encoded SHA-256 per turn |
| `chain_hash` | `String` | SHA-256 of concatenated turn hashes |

**Lifecycle**: Created via `AuditedInvocation::from_invocation(inv)`. Verified via `verify() -> bool`.
**Hash algorithm**: SHA-256 (via `sha2` crate). Each turn serialized to JSON, hashed individually, then concatenated hashes hashed for chain.

### EvalError (enum)

| Variant | Fields | Description |
|---------|--------|-------------|
| `Agent` | `source: AgentError` | Agent error during eval |
| `CaseNotFound` | `id: String` | Missing case |
| `SetNotFound` | `id: String` | Missing set |
| `InvalidCase` | `reason: String` | Invalid definition |
| `Io` | `source: io::Error` | Filesystem error |
| `Serde` | `source: serde_json::Error` | JSON error |
| `Yaml` | `source: serde_yaml::Error` | YAML error (feature-gated) |

## Relationships

```
EvalSet 1──* EvalCase
EvalRunner ──uses──> EvaluatorRegistry ──contains──* Arc<dyn Evaluator>
EvalRunner ──uses──> AgentFactory ──creates──> (Agent, CancellationToken)
EvalRunner ──produces──> EvalSetResult 1──* EvalCaseResult
EvalCaseResult 1──1 Invocation 1──* TurnRecord
EvalCaseResult 1──* EvalMetricResult 1──1 Score ──derives──> Verdict
EvalStore ──persists──> EvalSet, EvalSetResult
GateConfig + EvalSetResult ──check_gate()──> GateResult
Invocation ──wraps──> AuditedInvocation
BudgetGuard ──enforces──> BudgetConstraints (real-time)
BudgetEvaluator ──scores──> BudgetConstraints (post-hoc)
```

## Persistence Layout

```
{store_dir}/
├── sets/
│   ├── suite-a.json        # EvalSet definition
│   └── suite-b.json
└── results/
    ├── suite-a/
    │   ├── 1711211400.json  # EvalSetResult (timestamped)
    │   └── 1711297800.json
    └── suite-b/
        └── 1711211400.json
```
