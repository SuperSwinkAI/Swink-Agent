# AGENTS.md — swink-agent-patterns

## Scope

`patterns/` — Composable multi-agent pipeline primitives: sequential, parallel, loop. Feature-gated behind `pipelines` (default-enabled).

## Key Facts

- `Pipeline` → sequence of steps, each backed by `AgentFactory`. `PipelineExecutor` drives execution, emits `PipelineEvent`.
- `PipelineRegistry` — named collection. `PipelineTool` exposes a pipeline as `AgentTool`.
- `ExitCondition` — regex or custom predicate for loop termination. `MergeStrategy` for parallel output combination.
- `parallel.rs` must convert branch panics and silent exits into `PipelineError::StepFailed` — never `expect()` missing results.
