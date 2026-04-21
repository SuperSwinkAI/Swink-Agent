# swink-agent-eval

[![Crates.io](https://img.shields.io/crates/v/swink-agent-eval.svg)](https://crates.io/crates/swink-agent-eval)
[![Docs.rs](https://docs.rs/swink-agent-eval/badge.svg)](https://docs.rs/swink-agent-eval)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](https://github.com/SuperSwinkAI/Swink-Agent/blob/main/LICENSE-MIT)

Evaluation framework for [`swink-agent`](https://crates.io/crates/swink-agent) — trajectory tracing, golden-path matching, and cost/latency budget enforcement in one harness.

## Features

- **`EvalRunner`** — drives cases end-to-end against a user-provided `AgentFactory`
- **`TrajectoryMatcher`** — match expected tool-call sequences with exact, subset, or ordered modes (`MatchMode`)
- **`ResponseMatcher`** — assertions on the assistant's final text (substring, regex, semantic)
- **`BudgetEvaluator` / `EfficiencyEvaluator`** — per-case cost, token, and latency governance
- **`GateConfig`** — pass/fail gates on aggregate suite results (CI-friendly)
- **`FsEvalStore`** — persist trajectories and scores under a versioned directory layout
- **`yaml` feature** — load `EvalSet`s from YAML with `load_eval_set_yaml`
- **Audit log** (`AuditedInvocation`) — full request/response capture for replay and debugging

## Quick Start

```toml
[dependencies]
swink-agent = "0.8"
swink-agent-eval = { version = "0.8", features = ["yaml"] }
tokio = { version = "1", features = ["full"] }
```

```rust,ignore
use swink_agent_eval::{EvalRunner, EvalSet, AgentFactory};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let set = EvalSet {
        id: "demo".into(),
        name: "Demo".into(),
        description: None,
        cases: vec![/* EvalCase entries */],
    };

    let runner = EvalRunner::with_defaults();
    let result = runner.run_set(&set, &my_factory).await?;

    println!("Passed: {}/{}", result.summary.passed, result.summary.total_cases);
    Ok(())
}
```

## Architecture

A run is three staged components: a `TrajectoryCollector` captures every `AgentEvent` emitted by the loop, `Evaluator` implementations score the trajectory against an `EvalCase`'s expectations, and `EvalStore` persists the result. The runner drives cases in parallel with a `BudgetGuard` that can short-circuit a case when it exceeds cost or turn ceilings. Matchers are independent building blocks — you can run trajectory, response, and budget checks alone or compose them via `EvaluatorRegistry`.

No `unsafe` code (`#![forbid(unsafe_code)]`). Eval runs never mutate shared state outside the provided `EvalStore`.

---

Part of the [swink-agent](https://github.com/SuperSwinkAI/Swink-Agent) workspace — see the [main README](https://github.com/SuperSwinkAI/Swink-Agent#readme) for workspace overview and setup.
