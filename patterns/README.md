# swink-agent-patterns

[![Crates.io](https://img.shields.io/crates/v/swink-agent-patterns.svg)](https://crates.io/crates/swink-agent-patterns)
[![Docs.rs](https://docs.rs/swink-agent-patterns/badge.svg)](https://docs.rs/swink-agent-patterns)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](https://github.com/SuperSwinkAI/Swink-Agent/blob/main/LICENSE-MIT)

Multi-agent pipeline patterns for [`swink-agent`](https://crates.io/crates/swink-agent) — compose sequential, parallel, and loop pipelines over an agent factory.

## Features

- **`Pipeline::sequential`** — chain agents, optionally passing each step's output as the next step's context
- **`Pipeline::parallel`** — fan out to N agents concurrently with a `MergeStrategy` (`Concat`, `First`, `Fastest { n }`, `Custom { aggregator }`)
- **`Pipeline::loop_`** — iterate one agent until an `ExitCondition` (`ToolCalled`, `OutputContains` regex, or `MaxIterations`)
- **`PipelineExecutor`** — drives a pipeline via a pluggable `AgentFactory` with lifecycle events (`PipelineEvent`)
- **`PipelineRegistry`** — name → pipeline lookup for dynamic dispatch (e.g. routing tools to sub-pipelines)
- **`PipelineTool`** — expose a pipeline to the outer agent as a regular `AgentTool`
- **`SimpleAgentFactory`** — register agent builder fns by name for quick prototyping

## Quick Start

```toml
[dependencies]
swink-agent = "0.8"
swink-agent-patterns = "0.8"
tokio = { version = "1", features = ["full"] }
```

```rust,ignore
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use swink_agent_patterns::{
    Pipeline, PipelineExecutor, PipelineRegistry, SimpleAgentFactory,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut factory = SimpleAgentFactory::new();
    factory.register("researcher", || build_research_agent());
    factory.register("writer", || build_writer_agent());

    let registry = Arc::new(PipelineRegistry::new());
    let pipeline = Pipeline::sequential_with_context(
        "research-then-write",
        vec!["researcher".into(), "writer".into()],
    );
    let pipeline_id = pipeline.id().clone();
    registry.register(pipeline);

    let executor = PipelineExecutor::new(Arc::new(factory), registry);
    let output = executor
        .run(&pipeline_id, "Write me a brief on LLM agents.".into(), CancellationToken::new())
        .await?;
    println!("{}", output.final_response);
    Ok(())
}
```

## Architecture

`Pipeline` is an enum (`Sequential`, `Parallel`, `Loop`) of pure data — each variant references agent steps by name, not by instance. `PipelineExecutor` materializes agents on demand via the `AgentFactory` trait, so expensive setup (credentials, tool registration) happens once per run rather than once per step. `PipelineEvent` emits lifecycle hooks (`StepStarted`, `StepCompleted`, `PipelineCompleted`) so callers can stream progress into a UI or audit log without cracking the executor open.

No `unsafe` code (`#![forbid(unsafe_code)]`). Pipelines are `Clone + Serialize` — you can persist a pipeline definition to disk and recreate it.

---

Part of the [swink-agent](https://github.com/SuperSwinkAI/Swink-Agent) workspace — see the [main README](https://github.com/SuperSwinkAI/Swink-Agent#readme) for workspace overview and setup.
