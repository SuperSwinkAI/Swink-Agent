# Quickstart: Eval Runner, Scoring & Governance

**Feature**: 024-eval-runner-governance

## Prerequisites

- Rust 1.88+ with edition 2024
- `swink-agent-eval` crate in workspace

## Add Dependency

```toml
[dependencies]
swink-agent-eval = { path = "../eval" }
```

For YAML eval file support:
```toml
swink-agent-eval = { path = "../eval", features = ["yaml"] }
```

## Define an Eval Set (JSON)

Create `evals/sets/my-suite.json`:

```json
{
  "id": "my-suite",
  "name": "My Eval Suite",
  "cases": [
    {
      "id": "case-1",
      "name": "Basic greeting",
      "system_prompt": "You are a helpful assistant.",
      "user_messages": ["Say hello"],
      "expected_response": {
        "mode": "contains",
        "substring": "hello"
      },
      "budget": {
        "max_tokens": 1000,
        "max_cost": 0.01
      }
    }
  ]
}
```

## Implement AgentFactory

```rust
use swink_agent::{Agent, AgentConfig};
use swink_agent_eval::{AgentFactory, EvalCase, EvalError};
use tokio_util::sync::CancellationToken;

struct MyFactory {
    stream_fn: Arc<dyn StreamFn>,
}

impl AgentFactory for MyFactory {
    fn create_agent(
        &self,
        case: &EvalCase,
    ) -> Result<(Agent, CancellationToken), EvalError> {
        let cancel = CancellationToken::new();
        let config = AgentConfig::new(self.stream_fn.clone())
            .with_system_prompt(&case.system_prompt)
            .with_cancellation_token(cancel.clone());
        let agent = Agent::new(config);
        Ok((agent, cancel))
    }
}
```

## Run a Suite

```rust
use swink_agent_eval::{EvalRunner, FsEvalStore, EvalStore};

let store = FsEvalStore::new("./evals");
let eval_set = store.load_set("my-suite")?;

let runner = EvalRunner::with_defaults();
let result = runner.run_set(&eval_set, &my_factory).await?;

println!("Passed: {}/{}", result.summary.passed, result.summary.total_cases);

// Persist results
store.save_result(&result)?;
```

## Gate a CI/CD Pipeline

```rust
use swink_agent_eval::{GateConfig, check_gate};
use std::time::Duration;

let gate_config = GateConfig::new()
    .with_min_pass_rate(0.95)
    .with_max_cost(10.0)
    .with_max_duration(Duration::from_secs(300));

let gate_result = check_gate(&result, &gate_config);
println!("{}", gate_result.summary);

if !gate_result.passed {
    gate_result.exit(); // exits with code 1
}
```

## Register Custom Evaluators

```rust
use swink_agent_eval::{EvalRunner, EvaluatorRegistry, Evaluator, EvalCase, Invocation, EvalMetricResult, Score};

struct MyCustomEvaluator;

impl Evaluator for MyCustomEvaluator {
    fn name(&self) -> &'static str { "custom" }

    fn evaluate(&self, case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        let score = if invocation.turns.len() <= 3 {
            Score::pass()
        } else {
            Score::fail()
        };
        Some(EvalMetricResult {
            evaluator_name: "custom".to_string(),
            score,
            details: Some(format!("{} turns", invocation.turns.len())),
        })
    }
}

let mut registry = EvaluatorRegistry::with_defaults();
registry.register(MyCustomEvaluator);
let runner = EvalRunner::new(registry);
```

## Verify Audit Trail Integrity

```rust
use swink_agent_eval::AuditedInvocation;

let audited = AuditedInvocation::from_invocation(invocation);
assert!(audited.verify()); // true if no tampering

// Serialize, store, then later verify
let json = serde_json::to_string(&audited)?;
let loaded: AuditedInvocation = serde_json::from_str(&json)?;
assert!(loaded.verify());
```

## Run Tests

```bash
cargo test -p swink-agent-eval
cargo test -p swink-agent-eval --features yaml
```
