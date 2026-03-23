# Quickstart: Eval Trajectory & Matching

## Add dependency

```toml
[dependencies]
swink-agent-eval = { path = "../eval" }
```

## Collect a trajectory from an agent run

```rust
use swink_agent_eval::TrajectoryCollector;

// From a stream (most common):
let invocation = TrajectoryCollector::collect_from_stream(agent_event_stream).await;

// Incremental observation (e.g., via event subscriber):
let mut collector = TrajectoryCollector::new();
collector.observe(&event);  // call for each AgentEvent
let invocation = collector.finish();
```

## Collect with budget enforcement

```rust
use swink_agent_eval::{BudgetGuard, TrajectoryCollector};
use tokio_util::sync::CancellationToken;

let cancel = CancellationToken::new();
let guard = BudgetGuard::new(cancel.clone())
    .with_max_cost(1.0)       // $1 max
    .with_max_tokens(10_000)   // 10k tokens max
    .with_max_turns(5);        // 5 turns max

let invocation = TrajectoryCollector::collect_with_guard(stream, Some(guard)).await;
// The agent run is cancelled if any threshold is exceeded,
// but the invocation trace is always complete.
```

## Compare against a golden path

```rust
use swink_agent_eval::{TrajectoryMatcher, MatchMode, ExpectedToolCall, EvalCase};

let matcher = TrajectoryMatcher::in_order();  // default mode

// Or choose a specific mode:
let exact_matcher = TrajectoryMatcher::exact();
let any_order_matcher = TrajectoryMatcher::any_order();

// Define expected trajectory:
let expected = vec![
    ExpectedToolCall { tool_name: "read_file".into(), arguments: None },
    ExpectedToolCall { tool_name: "write_file".into(), arguments: None },
];

// Use with Evaluator trait:
use swink_agent_eval::Evaluator;
let result = matcher.evaluate(&case, &invocation);
// result is Some(EvalMetricResult) if case has expected_trajectory, None otherwise
```

## Score efficiency

```rust
use swink_agent_eval::{EfficiencyEvaluator, Evaluator};

let evaluator = EfficiencyEvaluator::new();  // threshold = 0.5
// Or with custom threshold:
let evaluator = EfficiencyEvaluator::new().with_threshold(0.8);

let result = evaluator.evaluate(&case, &invocation);
// Returns None if no tool calls were made
```

## Match response content

```rust
use swink_agent_eval::{ResponseCriteria, EvalCase};

// Exact match
let criteria = ResponseCriteria::Exact { expected: "42".into() };

// Substring
let criteria = ResponseCriteria::Contains { substring: "success".into() };

// Regex
let criteria = ResponseCriteria::Regex { pattern: r"\d+ files".into() };

// Custom (not serializable — set programmatically)
use std::sync::Arc;
use swink_agent_eval::Score;
let criteria = ResponseCriteria::Custom(Arc::new(|response: &str| {
    if response.contains("done") && response.len() < 200 {
        Score::pass()
    } else {
        Score::fail()
    }
}));
```

## Use the registry for combined evaluation

```rust
use swink_agent_eval::EvaluatorRegistry;

// Default registry includes: trajectory (InOrder), budget, response, efficiency
let registry = EvaluatorRegistry::with_defaults();
let results = registry.evaluate(&case, &invocation);

for result in &results {
    println!("{}: {:.2} ({})",
        result.evaluator_name,
        result.score.value,
        result.score.verdict().is_pass().then_some("PASS").unwrap_or("FAIL"),
    );
}
```
