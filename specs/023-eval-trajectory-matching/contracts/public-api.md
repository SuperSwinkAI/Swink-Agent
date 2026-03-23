# Public API Contract: swink-agent-eval (023 scope)

This documents the public API surface of `swink-agent-eval` relevant to spec 023 (trajectory collection, matching, efficiency, response). Items from spec 024 (runner, store, gate, audit) are listed for completeness but not contracted here.

## Re-exports from `lib.rs`

```rust
// 023 scope — contracted
pub use trajectory::{BudgetGuard, TrajectoryCollector};
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
```

## TrajectoryCollector

```rust
impl TrajectoryCollector {
    pub fn new() -> Self;
    pub fn observe(&mut self, event: &AgentEvent);
    pub fn finish(self) -> Invocation;
    pub async fn collect_from_stream(stream: impl Stream<Item = AgentEvent>) -> Invocation;
    pub async fn collect_with_guard(
        stream: impl Stream<Item = AgentEvent>,
        guard: Option<BudgetGuard>,
    ) -> Invocation;
}
impl Default for TrajectoryCollector { ... }
```

### Behavioral Contract
- `observe()` processes events incrementally. Only `AgentStart`, `BeforeLlmCall`, `TurnStart`, `ToolExecutionStart`, and `TurnEnd` are recorded; all others are silently ignored.
- `finish()` consumes the collector and returns a complete `Invocation`.
- `collect_from_stream()` is a convenience that calls `new()` + `observe()` for each event + `finish()`.
- `collect_with_guard()` additionally checks budget thresholds after each event and cancels the token if exceeded. Stream is fully drained regardless of cancellation.

## BudgetGuard

```rust
impl BudgetGuard {
    pub const fn new(cancel: CancellationToken) -> Self;
    pub const fn with_max_cost(self, max_cost: f64) -> Self;
    pub const fn with_max_tokens(self, max_tokens: u64) -> Self;
    pub const fn with_max_turns(self, max_turns: usize) -> Self;
    pub fn from_case(case: &EvalCase, cancel: CancellationToken) -> Option<Self>;
}
```

### Behavioral Contract
- `from_case()` returns `None` when the case has no budget constraints or all threshold fields are `None`.
- Thresholds are checked after each `observe()` call during `collect_with_guard()`.
- Cancellation is one-shot: once triggered, no further cancellation calls are made.

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

## ResponseMatcher

```rust
pub struct ResponseMatcher;
impl Evaluator for ResponseMatcher { ... }
```

### Behavioral Contract
- Returns `None` when `case.expected_response` is `None`.
- Uses `invocation.final_response` (falls back to `""` if `None`).
- `Custom` arm: panics are caught via `catch_unwind` and reported as `Score::fail()` with diagnostic message.
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
    pub fn with_defaults() -> Self;  // trajectory(InOrder) + budget + response + efficiency
    pub fn register(&mut self, evaluator: impl Evaluator + 'static);
    pub fn evaluate(&self, case: &EvalCase, invocation: &Invocation) -> Vec<EvalMetricResult>;
}
```

### Behavioral Contract
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
