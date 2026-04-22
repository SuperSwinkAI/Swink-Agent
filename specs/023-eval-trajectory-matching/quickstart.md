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

## v2 example: combined semantic + env-state evaluation (US5 + US6 + US7)

Spec 023's v2 surface introduces three new evaluators that layer onto the v1
defaults:

- **`SemanticToolSelectionEvaluator`** (US5) — LLM-as-judge verdict on whether
  each chosen tool was appropriate for the user goal.
- **`SemanticToolParameterEvaluator`** (US6) — LLM-as-judge verdict on whether
  tool-call arguments satisfy a declared natural-language intent.
- **`EnvironmentStateEvaluator`** (US7) — deterministic comparison of a
  captured environment snapshot against expected named states.

The example below wires all three into a single `EvalCase`, plugs in a
[`MockJudge`](https://docs.rs/swink-agent-eval) so it runs without a real
provider, and evaluates a hand-built `Invocation` via the registry. This
mirrors `eval/tests/registry_panic_isolation.rs` and is a good starting
template for authoring v2 cases.

```rust,ignore
use std::sync::Arc;

use swink_agent_eval::{
    EnvironmentState, EvalCase, EvaluatorRegistry, ExpectedToolCall,
    JudgeClient, JudgeVerdict, MockJudge, ResponseCriteria, Score,
    ToolIntent,
};

// 1. Build a MockJudge that always returns Pass. Plug your provider-backed
//    JudgeClient here in production (spec 043 ships concrete impls).
let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![
    JudgeVerdict {
        score: 1.0,
        pass: true,
        reason: Some("fetch_document reads the file the user asked for".into()),
        label: Some("equivalent".into()),
    },
    JudgeVerdict {
        score: 1.0,
        pass: true,
        reason: Some("arguments resolve to ./project-alpha/config.toml".into()),
        label: Some("satisfies-intent".into()),
    },
]));

// 2. Define a case that opts in to all three v2 evaluators.
let case = EvalCase {
    id: "v2-combined".into(),
    name: "US5 + US6 + US7 worked example".into(),
    description: Some("Semantic tool-selection + semantic tool-parameter + env-state.".into()),
    system_prompt: "You are a helpful assistant.".into(),
    user_messages: vec!["Read the config for project-alpha and summarise it.".into()],
    expected_trajectory: Some(vec![ExpectedToolCall {
        tool_name: "read_file".into(),
        arguments: None,
    }]),
    expected_response: Some(ResponseCriteria::Contains {
        substring: "project-alpha".into(),
    }),
    budget: None,
    evaluators: vec![],
    metadata: serde_json::Value::Null,
    // US7: declare the env-state we expect after the run.
    expected_environment_state: Some(vec![EnvironmentState {
        name: "created_file".into(),
        state: serde_json::json!("./project-alpha/summary.md"),
    }]),
    // US6: intent is judged against whichever tool the agent actually called.
    expected_tool_intent: Some(ToolIntent {
        intent: "read project-alpha config".into(),
        tool_name: None,
    }),
    // US5: opt in to semantic tool-selection scoring.
    semantic_tool_selection: true,
    // US7: supply the capture closure that produces the actual env-state.
    //     Runs after the agent completes; panics are caught and surfaced as
    //     Score::fail() per FR-014.
    state_capture: Some(Arc::new(|_invocation| {
        vec![EnvironmentState {
            name: "created_file".into(),
            state: serde_json::json!("./project-alpha/summary.md"),
        }]
    })),
};

// 3. Build a registry with v1 defaults + the v2 semantic evaluators.
let registry = EvaluatorRegistry::with_defaults_and_judge(judge);

// 4. In real usage you'd obtain the `invocation` from EvalRunner::run_case.
//    For illustration we assume it's already on hand (see earlier sections).
let results: Vec<_> = registry.evaluate(&case, &invocation);

// 5. Each v2 evaluator contributes one EvalMetricResult. The shape is:
//      EvalMetricResult {
//          evaluator_name: "semantic_tool_selection" | "semantic_tool_parameter"
//                        | "environment_state" | "response" | "trajectory" | ...,
//          score: Score { value: 0.0..=1.0, threshold: 0.5 },
//          details: Some(human-readable justification),
//      }
//
//    Overall case verdict is Pass iff every result passes — the runner
//    surfaces this as `EvalCaseResult { metric_results, verdict, .. }`.
for r in &results {
    println!("{}: {:.2}  {}", r.evaluator_name, r.score.value,
        r.details.as_deref().unwrap_or(""));
}
```

**Panic isolation.** Every evaluator in the registry is wrapped in
`std::panic::catch_unwind`. If the `state_capture` closure, a `Custom`
response matcher, or a `JudgeClient::judge` call panics, the offending
evaluator degrades to `Score::fail()` with the panic message captured in
`details` — the runner continues and returns a complete `EvalCaseResult`
(FR-014 / SC-008). See `eval/tests/registry_panic_isolation.rs` for the
cross-cutting integration test that verifies this contract end-to-end.
