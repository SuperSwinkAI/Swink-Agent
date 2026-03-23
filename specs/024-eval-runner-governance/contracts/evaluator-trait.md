# Contract: Evaluator Trait & Public API

**Feature**: 024-eval-runner-governance
**Date**: 2026-03-23

## Evaluator Trait

The primary extension point. Implementations score an invocation against an eval case.

```rust
pub trait Evaluator: Send + Sync {
    /// Unique name for filtering via EvalCase::evaluators.
    fn name(&self) -> &'static str;

    /// Score the invocation. Return None if not applicable to this case.
    fn evaluate(&self, case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult>;
}
```

**Contract**:
- `name()` must return a unique, stable `&'static str` — used for case-level filtering
- `evaluate()` returns `None` when the evaluator is not applicable (e.g., no expected trajectory)
- `evaluate()` must not panic — panics in custom evaluators are the caller's responsibility
- Thread-safety: `Send + Sync` required for shared registry usage

**Blanket impl**: `(&'static str, F)` where `F: Fn(&EvalCase, &Invocation) -> Option<EvalMetricResult>` — enables quick one-off evaluators without a full struct.

## AgentFactory Trait

Decouples agent creation from the runner.

```rust
pub trait AgentFactory: Send + Sync {
    fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError>;
}
```

**Contract**:
- Must return a fresh `Agent` + `CancellationToken` per case
- The `CancellationToken` is used by `BudgetGuard` for real-time abort
- Agent configuration (model, tools, system prompt) is the factory's responsibility

## EvalStore Trait

Persistence abstraction for eval sets and results.

```rust
pub trait EvalStore: Send + Sync {
    fn save_set(&self, set: &EvalSet) -> Result<(), EvalError>;
    fn load_set(&self, id: &str) -> Result<EvalSet, EvalError>;
    fn save_result(&self, result: &EvalSetResult) -> Result<(), EvalError>;
    fn load_result(&self, eval_set_id: &str, timestamp: u64) -> Result<EvalSetResult, EvalError>;
    fn list_results(&self, eval_set_id: &str) -> Result<Vec<u64>, EvalError>;
}
```

**Contract**:
- `save_set` / `save_result`: Overwrites if already exists
- `load_set`: Returns `EvalError::SetNotFound` if not found; with `yaml` feature, checks `.yaml`/`.yml` before `.json`
- `load_result`: Returns `EvalError::SetNotFound` if not found
- `list_results`: Returns empty `Vec` if no results exist (not an error)
- Results keyed by `(eval_set_id, timestamp)` pair

## Built-in Evaluators

| Name | Type | Returns None When | Score Logic |
|------|------|-------------------|-------------|
| `"trajectory"` | `TrajectoryMatcher` | No `expected_trajectory` | Match ratio by mode (Exact/InOrder/AnyOrder) |
| `"response"` | `ResponseMatcher` | No `expected_response` | Pass/fail by criteria |
| `"budget"` | `BudgetEvaluator` | No `budget` constraints | Pass if all constraints met |
| `"efficiency"` | `EfficiencyEvaluator` | Zero tool calls | 0.6 * dup_ratio + 0.4 * step_ratio |

## Gate Function

```rust
pub fn check_gate(result: &EvalSetResult, config: &GateConfig) -> GateResult;
```

**Contract**:
- Empty config (no thresholds) → always passes
- Zero cases with pass rate threshold → passes (rate = 1.0)
- All configured thresholds checked; all violations reported in `summary`
- `GateResult.exit_code`: 0 = pass, 1 = fail
- Deterministic: same inputs always produce same output
