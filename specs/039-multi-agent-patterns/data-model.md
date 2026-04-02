# Data Model: Multi-Agent Patterns Crate & Pipeline Primitives

**Feature**: 039-multi-agent-patterns  
**Date**: 2026-04-02

## Entities

### PipelineId

Unique identifier for a pipeline definition.

- **Type**: Newtype over `String`
- **Derives**: Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize
- **Construction**: `PipelineId::new(impl Into<String>)` or `PipelineId::generate()` (UUID v4)
- **Display**: Delegates to inner string

### Pipeline

Sum type representing a pipeline definition. Each variant is a distinct composition pattern.

- **Derives**: Clone, Debug, Serialize, Deserialize
- **Variants**:

#### Pipeline::Sequential

| Field | Type | Description |
|-------|------|-------------|
| id | PipelineId | Unique identifier (set at construction) |
| name | String | Human-readable name |
| steps | Vec\<String\> | Agent names in execution order |
| pass_context | bool | If true, accumulate user/assistant text messages across steps |

#### Pipeline::Parallel

| Field | Type | Description |
|-------|------|-------------|
| id | PipelineId | Unique identifier |
| name | String | Human-readable name |
| branches | Vec\<String\> | Agent names to run concurrently |
| merge_strategy | MergeStrategy | How to combine branch outputs |

#### Pipeline::Loop

| Field | Type | Description |
|-------|------|-------------|
| id | PipelineId | Unique identifier |
| name | String | Human-readable name |
| body | String | Agent name to run repeatedly |
| exit_condition | ExitCondition | When to stop iterating |
| max_iterations | usize | Safety cap (default 10) |

**Shared behavior**:
- `Pipeline::id(&self) -> &PipelineId` — returns the ID regardless of variant
- `Pipeline::name(&self) -> &str` — returns the name regardless of variant

### MergeStrategy

Controls parallel branch output aggregation.

- **Derives**: Clone, Debug, Serialize, Deserialize
- **Variants**:

| Variant | Fields | Description |
|---------|--------|-------------|
| Concat | separator: String | Join all outputs in declaration order |
| First | — | Return first branch to complete |
| Fastest | n: usize | Return first N branches to complete |
| Custom | aggregator: String | Pass all outputs to named aggregator agent |

### ExitCondition

Controls loop pipeline termination.

- **Derives**: Clone, Debug, Serialize (custom), Deserialize (custom) |
- **Variants**:

| Variant | Fields | Description |
|---------|--------|-------------|
| ToolCalled | tool_name: String | Exit when body agent invokes named tool |
| OutputContains | pattern: String, compiled: Regex (skip serde) | Exit when output matches regex |
| MaxIterations | — | Always run to max_iterations cap |

**Construction**: `ExitCondition::output_contains(pattern)` validates regex eagerly, returns `Result`.

### PipelineOutput

Structured result from pipeline execution.

| Field | Type | Description |
|-------|------|-------------|
| pipeline_id | PipelineId | Which pipeline produced this output |
| final_response | String | The pipeline's final text output |
| steps | Vec\<StepResult\> | Per-step telemetry |
| total_duration | Duration | Wall-clock time for entire pipeline |
| total_usage | Usage | Aggregated token usage across all steps |

### StepResult

Per-step execution telemetry.

| Field | Type | Description |
|-------|------|-------------|
| agent_name | String | Which agent ran this step |
| response | String | Agent's text output |
| duration | Duration | Wall-clock time for this step |
| usage | Usage | Token usage for this step |

### PipelineError

Typed error variants for pipeline execution failures.

| Variant | Fields | Description |
|---------|--------|-------------|
| AgentNotFound | name: String | Named agent not found in factory |
| PipelineNotFound | id: PipelineId | Pipeline ID not in registry |
| StepFailed | step_index: usize, agent_name: String, source: Box\<dyn Error\> | A step errored |
| MaxIterationsReached | iterations: usize | Loop hit safety cap without meeting exit condition |
| Cancelled | — | Cancellation token was triggered |
| InvalidExitCondition | message: String | Regex compilation or other construction failure |

### PipelineEvent

Events emitted during pipeline execution.

| Variant | Fields | Description |
|---------|--------|-------------|
| Started | pipeline_id, pipeline_name | Pipeline execution began |
| StepStarted | pipeline_id, step_index, agent_name | A step/branch began |
| StepCompleted | pipeline_id, step_index, agent_name, duration, usage | A step/branch completed |
| Completed | pipeline_id, total_duration, total_usage | Pipeline finished successfully |
| Failed | pipeline_id, error_message | Pipeline failed |

Each variant implements `to_emission() -> Emission` for integration with `AgentEvent::Custom`.

## Relationships

```
PipelineRegistry 1──* Pipeline
Pipeline 1──1 PipelineId
Pipeline::Parallel 1──1 MergeStrategy
Pipeline::Loop 1──1 ExitCondition
PipelineExecutor *──1 AgentFactory
PipelineExecutor *──1 PipelineRegistry
PipelineExecutor::run() ──> PipelineOutput (contains Vec<StepResult>)
PipelineTool *──1 PipelineExecutor (via Arc)
PipelineTool *──1 PipelineId
```

## State Transitions

### Pipeline Execution Lifecycle

```
Idle ──[run()]──> Started ──[step begins]──> StepRunning
StepRunning ──[step completes]──> StepDone ──[more steps?]──> StepRunning
StepRunning ──[step errors]──> Failed
StepDone ──[no more steps]──> Completed
Started/StepRunning ──[cancelled]──> Cancelled
```

For loop pipelines, StepDone checks exit condition before looping back to StepRunning.
For parallel pipelines, multiple StepRunning states exist concurrently.
