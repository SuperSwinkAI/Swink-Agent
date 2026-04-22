# AGENTS.md — swink-agent-patterns

## Scope

`patterns/` — Composable multi-agent pipeline primitives: sequential, parallel, and loop execution patterns. Feature-gated behind `pipelines` (default-enabled).

## Key Facts

- `Pipeline` — defines a sequence of steps, each backed by an `AgentFactory`.
- `PipelineExecutor` — drives execution; emits `PipelineEvent` stream.
- `PipelineRegistry` — named collection of pipelines; looked up by `PipelineId`.
- `PipelineTool` — exposes a pipeline as an `AgentTool` so agents can invoke pipelines as tools.
- `ExitCondition` — regex or custom predicate that terminates a loop-style pipeline step.
- `MergeStrategy` — controls how parallel step outputs are combined.
- `AgentFactory` / `SimpleAgentFactory` — sync factory trait for producing `Agent` instances per pipeline run.
- `parallel.rs` must convert branch panics and silent branch exits into typed `PipelineError::StepFailed` values; merge helpers must never `expect(...)` missing branch results.

## Build & Test

```bash
cargo build -p swink-agent-patterns
cargo test -p swink-agent-patterns --features testkit
cargo clippy -p swink-agent-patterns -- -D warnings
```
