# Public API Contract: swink-agent-eval (023 scope)

This documents the public API surface of `swink-agent-eval` relevant to spec 023 (trajectory collection, matching, efficiency, response, v2 semantic evaluators + env-state). Items from spec 024 (runner, store, gate, audit) are listed for completeness but not contracted here.

## Re-exports from `lib.rs`

```rust
// 023 v1 — contracted
pub use trajectory::TrajectoryCollector;
pub use types::{
    EvalCase, EvalCaseResult, EvalMetricResult, EvalSet, EvalSetResult,
    EvalSummary, ExpectedToolCall, Invocation, RecordedToolCall,
    ResponseCriteria, TurnRecord, BudgetConstraints,
};
pub use match_::{MatchMode, TrajectoryMatcher};
pub use efficiency::EfficiencyEvaluator;
pub use response::ResponseMatcher;
pub use evaluator::{Evaluator, EvaluatorRegistry};
pub use score::{Score, Verdict};
pub use error::EvalError;

// 023 v2 — contracted (Phases 8–12)
pub use judge::{JudgeClient, JudgeVerdict, JudgeError};
pub use types::{EnvironmentState, ToolIntent, StateCapture};
pub use semantic_tool_selection::SemanticToolSelectionEvaluator;
pub use semantic_tool_parameter::SemanticToolParameterEvaluator;
pub use environment_state::EnvironmentStateEvaluator;
pub use testing::MockJudge;   // test double — always public, no feature gate (per QA audit)
```

**Removed in Phase 13** (previously re-exported in v1): `BudgetGuard`. Budget enforcement now uses `BudgetPolicy` / `MaxTurnsPolicy` from `swink-agent-policies`, attached by the `AgentFactory`.

## TrajectoryCollector

```rust
impl TrajectoryCollector {
    pub fn new() -> Self;
    pub fn observe(&mut self, event: &AgentEvent);
    pub fn finish(self) -> Invocation;
    pub async fn collect_from_stream(stream: impl Stream<Item = AgentEvent>) -> Invocation;
}
impl Default for TrajectoryCollector { ... }
```

### Behavioral Contract
- `observe()` processes events incrementally. Only `AgentStart`, `BeforeLlmCall`, `TurnStart`, `ToolExecutionStart`, and `TurnEnd` are recorded; all others are silently ignored.
- `finish()` consumes the collector and returns a complete `Invocation`.
- `collect_from_stream()` is a convenience that calls `new()` + `observe()` for each event + `finish()`.
- **Phase 13 removal**: `collect_with_guard()` is removed. Budget enforcement moves to the agent loop via `BudgetPolicy` attached by the `AgentFactory`; the collector no longer performs cancellation.

## BudgetConstraints (reshaped in Phase 13)

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BudgetConstraints {
    pub max_cost: Option<f64>,
    pub max_input: Option<u64>,
    pub max_output: Option<u64>,
    pub max_turns: Option<usize>,
}

impl BudgetConstraints {
    pub fn to_policies(
        &self,
    ) -> (Option<swink_agent_policies::BudgetPolicy>, Option<swink_agent_policies::MaxTurnsPolicy>);
}
```

### Behavioral Contract
- `to_policies()` returns `(None, None)` when all fields are `None`.
- Returns `(Some(BudgetPolicy), None)` when only cost/token fields are set.
- Returns `(None, Some(MaxTurnsPolicy))` when only `max_turns` is set.
- Returns `(Some, Some)` when both categories are set.
- v1 fields `max_tokens` (combined) and `max_duration` (wall-clock) are **removed**. Callers relying on combined-token caps or wall-clock deadlines must migrate to `max_input`+`max_output` or compose their own cancellation outside 023.

## TrajectoryMatcher

```rust
impl TrajectoryMatcher {
    pub const fn new(mode: MatchMode) -> Self;
    pub const fn exact() -> Self;
    pub const fn in_order() -> Self;    // default
    pub const fn any_order() -> Self;
}
impl Evaluator for TrajectoryMatcher { ... }
```

### Behavioral Contract
- Returns `None` when `case.expected_trajectory` is `None`.
- Flattens all `RecordedToolCall`s across turns before comparison.
- `matches_expected()`: name must match; if `ExpectedToolCall.arguments` is `Some`, JSON equality is also required.
- Score = `matched / total_expected`, threshold = 1.0.

## EfficiencyEvaluator

```rust
impl EfficiencyEvaluator {
    pub const fn new() -> Self;           // threshold = 0.5
    pub const fn with_threshold(self, threshold: f64) -> Self;
}
impl Evaluator for EfficiencyEvaluator { ... }
```

### Behavioral Contract
- Returns `None` when total tool calls across all turns is 0.
- Duplicate ratio: `unique_calls / total_calls` (uniqueness by `(name, JSON-serialized arguments)`).
- Step ratio: `min(ideal_turns, actual_turns) / actual_turns`, where `ideal_turns = budget.max_turns.unwrap_or(unique_count.max(1))`.
- Composite: `0.6 * dup_ratio + 0.4 * step_ratio`.
- **Determinism**: For identical inputs, `evaluate()` produces byte-identical `EvalMetricResult` (SC-003).

## ResponseMatcher

```rust
pub struct ResponseMatcher;
impl Evaluator for ResponseMatcher { ... }
```

### Behavioral Contract
- Returns `None` when `case.expected_response` is `None`.
- Uses `invocation.final_response` (falls back to `""` if `None`).
- `Custom` arm: panics are caught via `catch_unwind` and reported as `Score::fail()` with diagnostic message (FR-008).
- Invalid regex: returns `Score::fail()` with compilation error message.

## Evaluator Trait

```rust
pub trait Evaluator: Send + Sync {
    fn name(&self) -> &'static str;
    fn evaluate(&self, case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult>;
}
```

## EvaluatorRegistry

```rust
impl EvaluatorRegistry {
    pub fn new() -> Self;
    pub fn with_defaults() -> Self;
    // v2 — Phase 8
    pub fn with_judge(client: Arc<dyn JudgeClient>) -> Self;
    pub fn with_defaults_and_judge(client: Arc<dyn JudgeClient>) -> Self;
    pub fn register(&mut self, evaluator: impl Evaluator + 'static);
    pub fn evaluate(&self, case: &EvalCase, invocation: &Invocation) -> Vec<EvalMetricResult>;
}
```

### Behavioral Contract
- `with_defaults()`: trajectory (InOrder) + budget + response + efficiency + **EnvironmentStateEvaluator** (deterministic, safe to default-register).
- `with_judge(client)`: empty registry + judge wired for semantic evaluators.
- `with_defaults_and_judge(client)`: `with_defaults()` + `SemanticToolSelectionEvaluator` + `SemanticToolParameterEvaluator`.
- `evaluate()` runs all registered evaluators. If `case.evaluators` is non-empty, only named evaluators run.
- Evaluators returning `None` are excluded from results.

## Score & Verdict

```rust
impl Score {
    pub const fn new(value: f64, threshold: f64) -> Self;  // both clamped to [0.0, 1.0]
    pub const fn pass() -> Self;   // value=1.0, threshold=0.5
    pub const fn fail() -> Self;   // value=0.0, threshold=0.5
    pub fn verdict(&self) -> Verdict;  // Pass if value >= threshold
}
```

---

## v2 Surface (Phases 8–12)

### JudgeClient, JudgeVerdict, JudgeError

```rust
#[async_trait]
pub trait JudgeClient: Send + Sync {
    async fn judge(&self, prompt: &str) -> Result<JudgeVerdict, JudgeError>;
}

#[derive(Debug, Clone)]
pub struct JudgeVerdict {
    pub score: f64,                 // clamped [0.0, 1.0]
    pub pass: bool,
    pub reason: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum JudgeError {
    #[error("transport: {0}")]        Transport(String),
    #[error("timeout")]               Timeout,
    #[error("malformed response: {0}")] MalformedResponse(String),
    #[error("other: {0}")]            Other(String),
}
```

### Behavioral Contract
- `JudgeClient` trait has no deadline parameter; concrete impls live in spec 043.
- All `JudgeError` variants map to `Score::fail()` with `details` containing the variant name and error context (FR-014).

### SemanticToolSelectionEvaluator

```rust
pub struct SemanticToolSelectionEvaluator {
    judge: Arc<dyn JudgeClient>,
    timeout: Duration,   // default: Duration::from_secs(300)
}

impl SemanticToolSelectionEvaluator {
    pub fn new(judge: Arc<dyn JudgeClient>) -> Self;  // timeout = 5 min
    pub fn with_timeout(self, timeout: Duration) -> Self;
}

impl Evaluator for SemanticToolSelectionEvaluator { ... }
```

### Behavioral Contract
- Returns `None` when `case.semantic_tool_selection == false` (FR-011).
- Iterates `invocation.turns[*].tool_calls`. For each call, wraps `judge.judge(prompt)` in `tokio::time::timeout(self.timeout, ...)`.
- Outer timeout elapse → `Score::fail()` with `details = "judge call exceeded {timeout:?}"` (FR-014).
- Inner `JudgeError::Timeout` from the impl → same `Score::fail()` treatment.
- Aggregates per-call verdicts into a single `Score` (mean of verdict.score values).

### SemanticToolParameterEvaluator

```rust
pub struct SemanticToolParameterEvaluator {
    judge: Arc<dyn JudgeClient>,
    timeout: Duration,   // default: Duration::from_secs(300)
}

impl SemanticToolParameterEvaluator {
    pub fn new(judge: Arc<dyn JudgeClient>) -> Self;
    pub fn with_timeout(self, timeout: Duration) -> Self;
}

impl Evaluator for SemanticToolParameterEvaluator { ... }
```

### Behavioral Contract
- Returns `None` when `case.expected_tool_intent.is_none()` (FR-012).
- If `expected_tool_intent.tool_name` is `Some(name)`, only calls matching that name are judged; other calls are skipped (not Pass, not Fail).
- Same timeout + panic-isolation contract as `SemanticToolSelectionEvaluator`.

### EnvironmentStateEvaluator

```rust
pub struct EnvironmentStateEvaluator;

impl Evaluator for EnvironmentStateEvaluator { ... }
```

### Behavioral Contract
- Returns `None` when `case.expected_environment_state.is_none()` OR `case.state_capture.is_none()` (FR-013).
- Invokes the registered `StateCapture` closure wrapped in `catch_unwind(AssertUnwindSafe(...))`.
- On panic → `Score::fail()` with panic message (FR-014).
- Compares each expected named state against captured state via full JSON equality.
- Missing name → Fail with missing-name details.
- Value mismatch → Fail with expected + actual JSON in details.
- Captured entries not declared in `expected_environment_state` are ignored.

### EnvironmentState, ToolIntent, StateCapture

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentState {
    pub name: String,
    pub state: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolIntent {
    pub intent: String,
    pub tool_name: Option<String>,
}

pub type StateCapture = Arc<dyn Fn(&Invocation) -> Vec<EnvironmentState> + Send + Sync>;
```

### EvalCase (v2 field additions)

```rust
pub struct EvalCase {
    // ... existing v1 fields ...

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_environment_state: Option<Vec<EnvironmentState>>,   // FR-013

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_tool_intent: Option<ToolIntent>,                    // FR-012

    #[serde(default, skip_serializing_if = "is_false")]
    pub semantic_tool_selection: bool,                               // FR-011

    #[serde(skip)]
    pub state_capture: Option<StateCapture>,                         // programmatic only
}
```

### Case-load Validation
- Duplicate names within `expected_environment_state` → `EvalError::InvalidCase { reason }` at case-load time pointing to the offending name (FR-015, SC-009).
