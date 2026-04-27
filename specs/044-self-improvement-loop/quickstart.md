# Quickstart: swink-agent-evolve

## Add the dependency

```toml
[dependencies]
swink-agent-evolve = { path = "../evolve" }
swink-agent-eval = { path = "../eval", features = ["judge-core"] }
swink-agent-eval-judges = { path = "../eval-judges", features = ["openai"] }
```

## Run a single optimization cycle

```rust
use swink_agent_evolve::{
    EvolutionRunner, OptimizationTarget, OptimizationConfig,
    CycleBudget, LlmGuided, TemplateBased, Ablation,
};
use swink_agent_eval::{EvalSet, EvalCase, Cost};
use swink_agent_eval_judges::OpenAIJudgeClient;
use std::sync::Arc;

// 1. Define your current agent configuration
let target = OptimizationTarget::new(
    "You are a helpful assistant that uses tools to answer questions.",
    vec![/* your tool schemas */],
);

// 2. Define eval cases that test the agent's behavior
let eval_set = EvalSet {
    id: "my-eval".into(),
    name: "Agent Quality".into(),
    description: "Tests for correct tool usage and response quality".into(),
    cases: vec![/* your EvalCase definitions */],
};

// 3. Set up the judge for LLM-guided mutations
let judge = Arc::new(OpenAIJudgeClient::new("gpt-4o"));

// 4. Configure the optimization cycle
let config = OptimizationConfig::new(eval_set, "./evolve-output")
    .with_strategies(vec![
        Box::new(LlmGuided::new(judge.clone())),
        Box::new(TemplateBased::new()),
        Box::new(Ablation::new()),
    ])
    .with_budget(CycleBudget::new(Cost::from_dollars(5.0)))
    .with_acceptance_threshold(0.02)
    .with_parallelism(2)
    .with_seed(42);

// 5. Run one cycle
let factory = Arc::new(MyAgentFactory::new(/* ... */));
let mut runner = EvolutionRunner::new(target, config, factory, Some(judge));
let result = runner.run_cycle().await?;

println!("Cycle {}: {} candidates evaluated, {} accepted",
    result.cycle_number,
    result.candidates_evaluated,
    result.acceptance.applied.len(),
);
```

## Run multiple cycles

```rust
// Run up to 5 cycles, stopping early if no improvements found
let results = runner.run_cycles(5).await?;

for result in &results {
    println!("Cycle {}: score {:.3} → status {:?}",
        result.cycle_number,
        result.baseline.aggregate_score,
        result.status,
    );
}
```

## Inspect the audit trail

```bash
# View all accepted mutations
cat evolve-output/cycle-0001-*/manifest.jsonl | jq 'select(.verdict == "Accepted")'

# Compare scores across cycles
for f in evolve-output/cycle-*/manifest.jsonl; do
  echo "$(dirname $f): $(jq -s '.[0].baseline_score' $f) → $(jq -s 'map(select(.verdict == "Accepted")) | .[0].candidate_score' $f)"
done
```

## Custom mutation strategy

```rust
use swink_agent_evolve::{MutationStrategy, MutationContext, Candidate, MutationError};

struct DomainSpecificMutator;

impl MutationStrategy for DomainSpecificMutator {
    fn name(&self) -> &str { "domain-specific" }

    fn mutate(&self, target: &str, context: &MutationContext) -> Result<Vec<Candidate>, MutationError> {
        // Your domain-specific rewrite logic
        let improved = target.replace("should", "must");
        Ok(vec![Candidate {
            id: sha256(&improved),
            component: context.weak_point.component.clone(),
            original_value: target.to_string(),
            mutated_value: improved,
            strategy: self.name().to_string(),
        }])
    }
}

let config = OptimizationConfig::new(eval_set, "./output")
    .with_strategies(vec![
        Box::new(DomainSpecificMutator),
        Box::new(TemplateBased::new()),
    ]);
```
