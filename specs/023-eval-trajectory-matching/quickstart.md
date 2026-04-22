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

## Budget enforcement (Phase 13 — via BudgetPolicy on the agent)

Budget caps are attached to the agent by the `AgentFactory`, not by the trajectory collector. `BudgetConstraints::to_policies()` converts an `EvalCase.budget` into the corresponding `BudgetPolicy` / `MaxTurnsPolicy` from `swink-agent-policies`:

```rust
use swink_agent::AgentOptions;
use swink_agent_eval::{AgentFactory, EvalCase, EvalError};
use swink_agent_eval::TrajectoryCollector;

struct MyFactory { /* model, tools, stream_fn, etc. */ }

impl AgentFactory for MyFactory {
    fn create_agent(&self, case: &EvalCase) -> Result<(swink_agent::Agent, tokio_util::sync::CancellationToken), EvalError> {
        let mut opts = AgentOptions::new_simple(/* ... */);

        // Attach policies derived from the case's budget, if any.
        if let Some(b) = case.budget.as_ref() {
            let (budget, max_turns) = b.to_policies();
            if let Some(p) = budget    { opts = opts.with_pre_turn_policy(p); }
            if let Some(p) = max_turns { opts = opts.with_pre_turn_policy(p); }
        }

        let agent = swink_agent::Agent::new(opts)?;
        Ok((agent, tokio_util::sync::CancellationToken::new()))
    }
}

// Trajectory collection itself is policy-free:
let invocation = TrajectoryCollector::collect_from_stream(stream).await;
```

**Capability note**: `BudgetPolicy` fires at turn boundaries only. Mid-turn cancellation and wall-clock `max_duration` (previously on `BudgetGuard`) are not supported in 023. Callers needing either must compose their own cancellation outside this crate.

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

## Worked semantic + environment example (US5 + US6 + US7)

```rust
use std::sync::Arc;

use swink_agent_eval::{
    EnvironmentState, EvalCase, EvaluatorRegistry, Invocation, MockJudge,
    ResponseCriteria, Score, ToolIntent,
};

let registry = EvaluatorRegistry::with_defaults_and_judge(Arc::new(MockJudge::always_pass()));

let case = EvalCase {
    id: "project-alpha".into(),
    name: "Semantic tool + response + env state".into(),
    description: None,
    system_prompt: "You are a careful assistant.".into(),
    user_messages: vec!["Read the project-alpha config and summarize it.".into()],
    expected_trajectory: None,
    expected_response: Some(ResponseCriteria::Custom(Arc::new(|response: &str| {
        if response.contains("project-alpha") && response.contains("config") {
            Score::pass()
        } else {
            Score::fail()
        }
    }))),
    budget: None,
    evaluators: vec![],
    metadata: serde_json::Value::Null,
    expected_environment_state: Some(vec![EnvironmentState {
        name: "config_path".into(),
        state: serde_json::json!("./project-alpha/config.toml"),
    }]),
    expected_tool_intent: Some(ToolIntent {
        intent: "read config for project-alpha".into(),
        tool_name: Some("read_file".into()),
    }),
    semantic_tool_selection: true,
    state_capture: Some(Arc::new(|invocation: &Invocation| {
        invocation
            .turns
            .iter()
            .flat_map(|turn| turn.tool_calls.iter())
            .find(|call| call.name == "read_file")
            .map(|call| {
                vec![EnvironmentState {
                    name: "config_path".into(),
                    state: call.arguments["path"].clone(),
                }]
            })
            .unwrap_or_default()
    })),
};

let results = registry.evaluate(&case, &invocation);

assert!(results.iter().any(|metric| metric.evaluator_name == "semantic_tool_selection"));
assert!(results.iter().any(|metric| metric.evaluator_name == "semantic_tool_parameter"));
assert!(results.iter().any(|metric| metric.evaluator_name == "environment_state"));
assert!(results.iter().any(|metric| metric.evaluator_name == "response"));
```

`state_capture` and `ResponseCriteria::Custom` are programmatic only, so keep
them in Rust rather than serialized eval-set fixtures. `MockJudge` lets the
quickstart demonstrate semantic scoring without a live provider dependency.
