# Public API Contract: swink-agent-patterns

**Feature**: 039-multi-agent-patterns  
**Date**: 2026-04-02

## Crate: `swink-agent-patterns`

### Feature Gates

| Feature | Default | Modules |
|---------|---------|---------|
| `pipelines` | yes | `pipeline::*` |
| `all` | no | Enables all features |

### Public Types (re-exported from `lib.rs`)

When `pipelines` feature is enabled:

```
// Pipeline definitions
Pipeline              — enum { Sequential, Parallel, Loop }
PipelineId            — newtype(String), Clone + Eq + Hash + Serialize
MergeStrategy         — enum { Concat, First, Fastest, Custom }
ExitCondition         — enum { ToolCalled, OutputContains, MaxIterations }

// Infrastructure
PipelineRegistry      — thread-safe pipeline storage
PipelineExecutor      — stateless execution coordinator
AgentFactory          — trait for creating agents by name
SimpleAgentFactory    — HashMap-backed AgentFactory implementation

// Results
PipelineOutput        — execution result with per-step telemetry
StepResult            — per-step agent name, response, duration, usage
PipelineError         — typed error variants

// Events
PipelineEvent         — enum of lifecycle events

// Tool bridge
PipelineTool          — AgentTool implementation wrapping a pipeline
```

### AgentFactory Trait

```
trait AgentFactory: Send + Sync {
    fn create(&self, name: &str) -> Result<Agent, PipelineError>;
}
```

Implementors: `SimpleAgentFactory`, `Arc<dyn Fn(&str) -> Result<Agent, PipelineError> + Send + Sync>`

### PipelineRegistry API

```
PipelineRegistry::new() -> Self
PipelineRegistry::register(&self, pipeline: Pipeline)
PipelineRegistry::get(&self, id: &PipelineId) -> Option<Pipeline>
PipelineRegistry::list(&self) -> Vec<(PipelineId, String)>
PipelineRegistry::remove(&self, id: &PipelineId) -> Option<Pipeline>
PipelineRegistry::len(&self) -> usize
PipelineRegistry::is_empty(&self) -> bool
```

### PipelineExecutor API

```
PipelineExecutor::new(
    agent_factory: Arc<dyn AgentFactory>,
    registry: Arc<PipelineRegistry>,
) -> Self

PipelineExecutor::with_event_handler(
    self,
    handler: Arc<dyn Fn(PipelineEvent) + Send + Sync>,
) -> Self

PipelineExecutor::run(
    &self,
    pipeline_id: &PipelineId,
    input: String,
    cancellation_token: CancellationToken,
) -> Result<PipelineOutput, PipelineError>
```

### Pipeline Constructors

```
// Sequential
Pipeline::sequential(name: impl Into<String>, steps: Vec<String>) -> Self
Pipeline::sequential_with_context(name: impl Into<String>, steps: Vec<String>) -> Self

// Parallel
Pipeline::parallel(name: impl Into<String>, branches: Vec<String>, strategy: MergeStrategy) -> Self

// Loop
Pipeline::loop_(name: impl Into<String>, body: impl Into<String>, exit: ExitCondition) -> Result<Self, PipelineError>
Pipeline::loop_with_max(name: impl Into<String>, body: impl Into<String>, exit: ExitCondition, max: usize) -> Result<Self, PipelineError>

// Common
Pipeline::with_id(self, id: PipelineId) -> Self
```

### PipelineTool Constructor

```
PipelineTool::new(
    pipeline_id: PipelineId,
    executor: Arc<PipelineExecutor>,
) -> Self

PipelineTool::with_description(self, description: impl Into<String>) -> Self
```

### Trait Implementations

| Type | Traits |
|------|--------|
| Pipeline | Clone, Debug, Serialize, Deserialize |
| PipelineId | Clone, Debug, PartialEq, Eq, Hash, Display, Serialize, Deserialize |
| MergeStrategy | Clone, Debug, Serialize, Deserialize |
| ExitCondition | Clone, Debug, Serialize (custom), Deserialize (custom) |
| PipelineOutput | Clone, Debug |
| StepResult | Clone, Debug |
| PipelineError | Debug, Display, Error |
| PipelineEvent | Clone, Debug, Serialize |
| PipelineRegistry | Clone (Arc internals) |
| PipelineExecutor | Clone (Arc internals) |
| PipelineTool | AgentTool |

### Dependencies on `swink-agent` Public API

| Type Used | From Module |
|-----------|-------------|
| Agent | `swink_agent::Agent` |
| AgentOptions | `swink_agent::AgentOptions` |
| AgentTool | `swink_agent::AgentTool` |
| AgentToolResult | `swink_agent::AgentToolResult` |
| Usage | `swink_agent::Usage` |
| Emission | `swink_agent::Emission` |
| AgentEvent | `swink_agent::AgentEvent` (for doc references only) |
| CancellationToken | `tokio_util::CancellationToken` |
