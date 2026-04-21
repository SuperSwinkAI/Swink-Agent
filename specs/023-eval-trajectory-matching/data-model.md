# Data Model: Eval Trajectory & Matching

## Entity Relationship Diagram

```text
EvalCase 1──* ExpectedToolCall          (golden path definition)
EvalCase 1──? ResponseCriteria          (optional response check)
EvalCase 1──? BudgetConstraints         (v2: declarative — attaches BudgetPolicy/MaxTurnsPolicy via AgentFactory)
EvalCase 1──? Vec<EnvironmentState>     (v2: expected_environment_state, FR-013)
EvalCase 1──? ToolIntent                (v2: expected_tool_intent, FR-012)
EvalCase 1──? StateCapture              (v2: programmatic callback, FR-013)
EvalCase 1──1 bool                      (v2: semantic_tool_selection flag, FR-011)

TrajectoryCollector ──observes──> AgentEvent stream
TrajectoryCollector ──produces──> Invocation

Invocation 1──* TurnRecord              (ordered by turn_index)
TurnRecord 1──* RecordedToolCall        (ordered by execution)
TurnRecord 1──* ToolResultMessage       (from core crate)
TurnRecord 1──1 AssistantMessage        (from core crate)

[Phase 13: BudgetGuard removed. Budget enforcement moves to the agent loop via
 BudgetPolicy (PreTurn slot) and MaxTurnsPolicy from swink-agent-policies,
 attached to the agent by the AgentFactory based on EvalCase.budget.]

TrajectoryMatcher ──compares──> (Invocation, EvalCase.expected_trajectory)
TrajectoryMatcher ──produces──> EvalMetricResult

EfficiencyEvaluator ──scores──> (Invocation, EvalCase.budget.max_turns)
EfficiencyEvaluator ──produces──> EvalMetricResult

ResponseMatcher ──matches──> (Invocation.final_response, EvalCase.expected_response)
ResponseMatcher ──produces──> EvalMetricResult

SemanticToolSelectionEvaluator ──judges──> (RecordedToolCall, JudgeClient)   [v2]
SemanticToolParameterEvaluator  ──judges──> (RecordedToolCall, JudgeClient)   [v2]
EnvironmentStateEvaluator       ──compares──> (EnvironmentState, expected)    [v2]

JudgeClient ──returns──> Result<JudgeVerdict, JudgeError>
JudgeVerdict ──derives──> Score

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

### BudgetConstraints (reshaped in Phase 13)
| Field | Type | Constraints | Maps to |
|---|---|---|---|
| `max_cost` | `Option<f64>` | Dollars | `BudgetPolicy::max_cost(f64)` |
| `max_input` | `Option<u64>` | Input tokens | `BudgetPolicy::max_input(u64)` |
| `max_output` | `Option<u64>` | Output tokens | `BudgetPolicy::max_output(u64)` |
| `max_turns` | `Option<usize>` | Turn count | `MaxTurnsPolicy::new(u64)` |

**Note**: v1 fields `max_tokens` (combined) and `max_duration` (wall-clock) are dropped in Phase 13. `BudgetPolicy` splits tokens into input/output; wall-clock deadlines are no longer enforced by 023. Callers needing either must compose their own (outside 023's surface).

**Helper**: `BudgetConstraints::to_policies(&self) -> (Option<BudgetPolicy>, Option<MaxTurnsPolicy>)` — factory implementers call this to derive policies to attach via `AgentOptions::with_pre_turn_policy(...)`. Returns `(None, None)` when all fields are `None`.

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

---

## v2 Entities (Phases 8–12)

### JudgeClient (trait)
```rust
#[async_trait]
pub trait JudgeClient: Send + Sync {
    async fn judge(&self, prompt: &str) -> Result<JudgeVerdict, JudgeError>;
}
```

| Aspect | Contract |
|---|---|
| Shape | Single async method; no deadline parameter on the trait |
| Implementations | Out of scope for 023; concrete bindings live in spec 043 (`043-evals-adv-features`) |
| Test double | `MockJudge` in `eval/src/testing.rs` (always public, no feature gate) — returns canned `JudgeVerdict` or `JudgeError` sequences |
| Deadline enforcement | Evaluator-side via `tokio::time::timeout` (default 5 min, overridable per evaluator) — see `SemanticToolSelectionEvaluator` / `SemanticToolParameterEvaluator` |

### JudgeVerdict
| Field | Type | Constraints |
|---|---|---|
| `score` | `f64` | Clamped to `[0.0, 1.0]` — maps directly to `Score.value` |
| `pass` | `bool` | Judge's own pass/fail determination (overrides threshold comparison when provided) |
| `reason` | `Option<String>` | Human-readable justification, surfaced in `EvalMetricResult.details` |
| `label` | `Option<String>` | Optional category label (e.g., "equivalent", "unrelated") |

Mirrors `strands-evals::EvaluationOutput` so future provider implementations map cleanly.

### JudgeError (enum)
| Variant | Meaning |
|---|---|
| `Transport` | Network/transport failure (connection refused, DNS, etc.) |
| `Timeout` | Inner deadline fired inside the concrete `JudgeClient` impl |
| `MalformedResponse` | Response parsed successfully but violates the verdict schema |
| `Other(String)` | Catch-all with diagnostic string |

All variants map to `Score::fail()` with the variant + context in `EvalMetricResult.details` (FR-014).

### ToolIntent
| Field | Type | Constraints |
|---|---|---|
| `intent` | `String` | Natural-language description of what the tool call should accomplish |
| `tool_name` | `Option<String>` | When `Some`, only calls matching this tool name are judged; other calls are skipped (not Pass, not Fail) |

### EnvironmentState
| Field | Type | Constraints |
|---|---|---|
| `name` | `String` | Identifier for this state entry. Duplicate names rejected at case-load (FR-015, SC-009) |
| `state` | `serde_json::Value` | Expected value; compared for full JSON equality against captured state |

### StateCapture (type alias)
```rust
pub type StateCapture = Arc<dyn Fn(&Invocation) -> Vec<EnvironmentState> + Send + Sync>;
```

| Aspect | Contract |
|---|---|
| Registration | On the `EvalCase` (via setter/builder) or supplied by the `AgentFactory` |
| Invocation | Once, after the agent completes and the `Invocation` is finalized |
| Panic handling | Caught via `catch_unwind(AssertUnwindSafe(...))`; panic → `Score::fail()` with panic message (FR-014) |
| Serialization | `#[serde(skip)]` on `EvalCase` — programmatic only, consistent with `ResponseCriteria::Custom` |

### SemanticToolSelectionEvaluator
| Field | Type | Default | Purpose |
|---|---|---|---|
| `judge` | `Arc<dyn JudgeClient>` | — | Injected via `EvaluatorRegistry::with_judge(...)` |
| `timeout` | `Duration` | `Duration::from_secs(300)` (5 min) | Per-judge-call outer deadline; configurable via `with_timeout(Duration)` |

**Applicability**: Returns `None` when `case.semantic_tool_selection == false` (FR-011). When applicable, iterates `invocation.turns[*].tool_calls`, wraps each `judge.judge(prompt)` in `tokio::time::timeout(self.timeout, ...)`, aggregates verdicts into a single `Score`. Outer timeout elapse → `Score::fail()` with timeout context (FR-014).

### SemanticToolParameterEvaluator
| Field | Type | Default | Purpose |
|---|---|---|---|
| `judge` | `Arc<dyn JudgeClient>` | — | Injected via `EvaluatorRegistry::with_judge(...)` |
| `timeout` | `Duration` | `Duration::from_secs(300)` (5 min) | Same contract as `SemanticToolSelectionEvaluator` |

**Applicability**: Returns `None` when `case.expected_tool_intent.is_none()` (FR-012). When `tool_intent.tool_name` is set, only calls matching that name are judged; non-matching calls are skipped.

### EnvironmentStateEvaluator
Stateless evaluator. No fields. Reads `case.expected_environment_state` and `case.state_capture`; runs the capture closure under `catch_unwind`; compares each expected named state against the captured state via full JSON equality. Returns `None` when either the expected list or the capture callback is absent (FR-013). Deterministic — no LLM dependency — safe to register in `EvaluatorRegistry::with_defaults()`.

### EvaluatorRegistry (v2 extensions)
New constructors introduced in Phase 8:
- `with_judge(client: Arc<dyn JudgeClient>) -> Self` — empty registry with judge wired in.
- `with_defaults_and_judge(client: Arc<dyn JudgeClient>) -> Self` — v1 defaults + `SemanticToolSelectionEvaluator` + `SemanticToolParameterEvaluator` + `EnvironmentStateEvaluator` (the deterministic env-state evaluator is also added to plain `with_defaults()`).

`with_defaults()` keeps current v1 behavior — the semantic evaluators are simply absent when no judge is configured, so existing callers see no change.
