# Data Model: Eval Trajectory & Matching

## Entity Relationship Diagram

```text
EvalCase 1──* ExpectedToolCall        (golden path definition)
EvalCase 1──? ResponseCriteria        (optional response check)
EvalCase 1──? BudgetConstraints       (optional budget limits)

TrajectoryCollector ──observes──> AgentEvent stream
TrajectoryCollector ──produces──> Invocation

Invocation 1──* TurnRecord            (ordered by turn_index)
TurnRecord 1──* RecordedToolCall      (ordered by execution)
TurnRecord 1──* ToolResultMessage     (from core crate)
TurnRecord 1──1 AssistantMessage      (from core crate)

BudgetGuard ──monitors──> TrajectoryCollector (accumulated metrics)
BudgetGuard ──cancels──> CancellationToken

TrajectoryMatcher ──compares──> (Invocation, EvalCase.expected_trajectory)
TrajectoryMatcher ──produces──> EvalMetricResult

EfficiencyEvaluator ──scores──> (Invocation, EvalCase.budget)
EfficiencyEvaluator ──produces──> EvalMetricResult

ResponseMatcher ──matches──> (Invocation.final_response, EvalCase.expected_response)
ResponseMatcher ──produces──> EvalMetricResult

EvalMetricResult *──1 Score
Score ──derives──> Verdict
```

## Entity Details

### RecordedToolCall
| Field | Type | Constraints |
|---|---|---|
| `id` | `String` | Provider-assigned tool call ID |
| `name` | `String` | Tool name as invoked |
| `arguments` | `serde_json::Value` | Parsed JSON arguments |

**Identity**: Unique within a turn by `id`. Across turns, `(turn_index, id)` is unique.

### TurnRecord
| Field | Type | Constraints |
|---|---|---|
| `turn_index` | `usize` | Zero-based, monotonically increasing |
| `assistant_message` | `AssistantMessage` | From core crate. Contains content, usage, cost, stop_reason |
| `tool_calls` | `Vec<RecordedToolCall>` | In execution order (event-stream arrival) |
| `tool_results` | `Vec<ToolResultMessage>` | Corresponding results from core |
| `duration` | `Duration` | Wall-clock time for this turn |

**Identity**: Unique within an Invocation by `turn_index`.

### Invocation
| Field | Type | Constraints |
|---|---|---|
| `turns` | `Vec<TurnRecord>` | Ordered by `turn_index` |
| `total_usage` | `Usage` | Sum of all turn usages |
| `total_cost` | `Cost` | Sum of all turn costs |
| `total_duration` | `Duration` | Wall-clock from AgentStart to finish |
| `final_response` | `Option<String>` | Extracted text from last turn's assistant message |
| `stop_reason` | `StopReason` | From last turn |
| `model` | `ModelSpec` | From first BeforeLlmCall event |

**Lifecycle**: Created empty by `TrajectoryCollector::new()` → populated incrementally via `observe()` → finalized via `finish()`.

### ExpectedToolCall
| Field | Type | Constraints |
|---|---|---|
| `tool_name` | `String` | Required. Matched against `RecordedToolCall.name` |
| `arguments` | `Option<Value>` | If `Some`, exact JSON equality required. If `None`, name-only match |

### ResponseCriteria (enum)
| Variant | Fields | Serialization |
|---|---|---|
| `Exact` | `expected: String` | JSON with `mode: "exact"` |
| `Contains` | `substring: String` | JSON with `mode: "contains"` |
| `Regex` | `pattern: String` | JSON with `mode: "regex"` |
| `Custom` | `Arc<dyn Fn(&str) -> Score>` | `#[serde(skip)]` — programmatic only |

### BudgetGuard
| Field | Type | Constraints |
|---|---|---|
| `cancel` | `CancellationToken` | Cancelled when any threshold exceeded |
| `max_cost` | `Option<f64>` | Dollars |
| `max_tokens` | `Option<u64>` | Total tokens (input + output) |
| `max_turns` | `Option<usize>` | Turn count |

**Lifecycle**: Created from `EvalCase.budget` via `from_case()` → checked after each `observe()` call → cancels token on threshold breach. Stream continues draining after cancellation.

### MatchMode (enum)
| Variant | Behavior |
|---|---|
| `Exact` | Same tools, same order, same count. No extras allowed. |
| `InOrder` | Expected tools in order. Extras between allowed. **Default.** |
| `AnyOrder` | All expected tools present. Order irrelevant. |

### Score
| Field | Type | Constraints |
|---|---|---|
| `value` | `f64` | Clamped to `[0.0, 1.0]` |
| `threshold` | `f64` | Clamped to `[0.0, 1.0]`, default `0.5` |

**Derived**: `verdict()` → `Pass` if `value >= threshold`, else `Fail`.

### Verdict (enum)
| Variant | Meaning |
|---|---|
| `Pass` | Score met threshold |
| `Fail` | Score below threshold |

### EvalMetricResult
| Field | Type | Constraints |
|---|---|---|
| `evaluator_name` | `String` | Matches `Evaluator::name()` |
| `score` | `Score` | Numeric result |
| `details` | `Option<String>` | Human-readable explanation |
