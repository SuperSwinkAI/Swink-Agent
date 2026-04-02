# Quickstart: swink-agent-patterns

**Feature**: 039-multi-agent-patterns  
**Date**: 2026-04-02

## Add the dependency

```toml
[dependencies]
swink-agent-patterns = { path = "../patterns" }
```

## Sequential Pipeline

```rust
use swink_agent_patterns::{
    Pipeline, PipelineExecutor, PipelineRegistry, SimpleAgentFactory,
};

// 1. Create an agent factory with named agent configurations
let factory = SimpleAgentFactory::new();
factory.register("researcher", || AgentOptions::new(
    "You are a research agent.",
    model.clone(),
    stream_fn.clone(),
    default_convert,
));
factory.register("writer", || AgentOptions::new(
    "You are a writing agent. Rewrite the research into a report.",
    model.clone(),
    stream_fn.clone(),
    default_convert,
));

// 2. Define the pipeline
let pipeline = Pipeline::sequential("research-to-report", vec![
    "researcher".into(),
    "writer".into(),
]);

// 3. Register and execute
let registry = PipelineRegistry::new();
registry.register(pipeline);

let executor = PipelineExecutor::new(
    Arc::new(factory),
    Arc::new(registry),
);

let output = executor.run(
    &PipelineId::new("research-to-report"),
    "What are the latest trends in AI safety?".into(),
    CancellationToken::new(),
).await?;

println!("Final report: {}", output.final_response);
println!("Total tokens: {}", output.total_usage.total);
```

## Parallel Pipeline

```rust
// Fan out to multiple analysts, merge results
let pipeline = Pipeline::parallel(
    "multi-analysis",
    vec!["sentiment-agent".into(), "fact-checker".into(), "style-agent".into()],
    MergeStrategy::Concat { separator: "\n\n---\n\n".into() },
);

let output = executor.run(&pipeline.id(), input, token).await?;
// output.final_response contains all three analyses joined by separator
```

## Loop Pipeline

```rust
// Self-correcting code generation
let pipeline = Pipeline::loop_with_max(
    "code-refiner",
    "coder",                                          // body agent name
    ExitCondition::ToolCalled("tests_pass".into()),   // stop when this tool is called
    5,                                                // max 5 attempts
)?;

let output = executor.run(&pipeline.id(), "Write a sorting function", token).await?;
// output.steps.len() tells you how many iterations it took
```

## Pipeline as Tool

```rust
use swink_agent_patterns::PipelineTool;

// Wrap a pipeline so a supervisor agent can invoke it
let tool = PipelineTool::new(
    PipelineId::new("research-to-report"),
    Arc::new(executor),
).with_description("Run the research-to-report pipeline");

// Add to supervisor agent's tool list
let supervisor = Agent::new(AgentOptions::new(
    "You orchestrate research tasks.",
    model, stream_fn, convert,
).with_tool(Arc::new(tool)));
```

## Event Observation

```rust
let executor = PipelineExecutor::new(factory, registry)
    .with_event_handler(Arc::new(|event: PipelineEvent| {
        match &event {
            PipelineEvent::StepCompleted { agent_name, duration, .. } => {
                println!("{} completed in {:?}", agent_name, duration);
            }
            _ => {}
        }
    }));
```

## Custom Merge with Aggregator Agent

```rust
factory.register("synthesizer", || AgentOptions::new(
    "Synthesize the following analyses into a unified summary.",
    model.clone(), stream_fn.clone(), convert,
));

let pipeline = Pipeline::parallel(
    "synthesized-analysis",
    vec!["agent-a".into(), "agent-b".into(), "agent-c".into()],
    MergeStrategy::Custom { aggregator: "synthesizer".into() },
);
// Aggregator receives: "[agent-a]: ...\n\n[agent-b]: ...\n\n[agent-c]: ..."
```
