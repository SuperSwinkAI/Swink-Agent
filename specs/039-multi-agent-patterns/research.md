# Research: Multi-Agent Patterns Crate & Pipeline Primitives

**Feature**: 039-multi-agent-patterns  
**Date**: 2026-04-02

## Research Tasks

### R1: Agent Instantiation for Pipeline Steps

**Question**: How does the executor create fresh agent instances when `Agent` and `AgentOptions` are not `Clone` (due to `Box<dyn Fn>` closure fields)?

**Finding**: `AgentOptions` contains `Box<ConvertToLlmFn>`, `Box<GetApiKeyFn>`, `Box<ApproveToolFn>`, and `Option<Box<dyn Fn() -> String>>` (dynamic system prompt) — none of which are `Clone`. However, most fields use `Arc` (tools, policies, stream_fn) which are cloneable. Directly cloning `AgentOptions` is not possible without upstream changes.

**Decision**: Introduce an `AgentFactory` trait in the patterns crate.

```
trait AgentFactory: Send + Sync {
    fn create(&self, name: &str) -> Result<Agent, PipelineError>;
}
```

The `PipelineExecutor` holds `Arc<dyn AgentFactory>` instead of `Arc<AgentRegistry>` directly. This decouples the executor from how agents are created. Consumers provide their own factory — the simplest implementation is a closure or struct that calls `Agent::new(AgentOptions { ... })` with the right config for each agent name.

A convenience type `RegistryAgentFactory` can wrap `Arc<AgentRegistry>` + a `HashMap<String, Arc<dyn Fn() -> AgentOptions + Send + Sync>>` for common use cases.

**Rationale**: This avoids modifying core types. It follows the "depends only on public API" constraint. It's more flexible than direct registry coupling — consumers can implement dynamic agent creation, pooling, or lazy initialization.

**Alternatives considered**:
- Make `AgentOptions` Clone (requires `Arc`-wrapping all closure fields in core — breaking change, rejected)
- Store agent factories in `AgentRegistry` (modifies core API — rejected per FR-001)
- Have executor lock `AgentRef` and "run in place" (defeats fresh-instance requirement — rejected)

### R2: Event Emission Mechanism

**Question**: How should the executor's optional event callback be typed?

**Finding**: The existing event system uses `Emission { name: String, payload: serde_json::Value }` via `AgentEvent::Custom(Emission)`. The executor needs to emit `Emission` values when a listener is configured.

**Decision**: The executor accepts `Option<Arc<dyn Fn(PipelineEvent) + Send + Sync>>` at construction. `PipelineEvent` is a crate-local enum that the callback receives. The crate also provides a `PipelineEvent::to_emission(&self) -> Emission` method for consumers who want to forward into the `AgentEvent::Custom` system.

**Rationale**: Using a crate-local enum gives type safety within the patterns crate. The `to_emission()` bridge keeps it compatible with the core event system without depending on it. Consumers wire the bridge: `|event| agent.dispatch_event(AgentEvent::Custom(event.to_emission()))`.

**Alternatives considered**:
- Accept `Fn(Emission)` directly (loses type safety for pipeline-specific events — rejected)
- Accept `mpsc::Sender<PipelineEvent>` (too prescriptive about channel type — rejected)
- Trait object `dyn PipelineEventSink` (unnecessary indirection for a single method — rejected)

### R3: Crate Structure Pattern

**Question**: How should the crate be organized following existing patterns?

**Finding**: Both `policies/` and `adapters/` use the same pattern:
- `Cargo.toml`: `swink-agent = { path = ".." }`, workspace deps, `default = []`, feature-per-module
- `lib.rs`: `#![forbid(unsafe_code)]`, paired `#[cfg(feature)]` on mod + pub use
- One file per major type, shared internals in private modules
- Version matches parent (0.4.2)

**Decision**: Follow the policies crate pattern exactly. `default = ["pipelines"]` (default-on unlike policies, because pipelines are the primary feature of this crate). Module layout:

```
patterns/
├── Cargo.toml
└── src/
    ├── lib.rs           # Feature-gated re-exports
    └── pipeline/        # Module directory (expected to exceed 800 lines)
        ├── mod.rs       # Re-exports from submodules
        ├── types.rs     # Pipeline, PipelineId, MergeStrategy, ExitCondition
        ├── registry.rs  # PipelineRegistry
        ├── executor.rs  # PipelineExecutor, AgentFactory trait
        ├── output.rs    # PipelineOutput, StepResult, PipelineError
        ├── events.rs    # PipelineEvent enum, to_emission()
        └── tool.rs      # PipelineTool (AgentTool impl)
```

### R4: Serialization of ExitCondition with Compiled Regex

**Question**: FR-010 requires regex compiled at construction, but FR-019 requires Pipeline to be serializable. `regex::Regex` does not implement `Serialize`/`Deserialize`.

**Finding**: Common pattern is to serialize the regex pattern string and recompile on deserialization. Use `#[serde(serialize_with, deserialize_with)]` or a newtype wrapper.

**Decision**: `ExitCondition::OutputContains` stores both the pattern string (for serialization) and a compiled `regex::Regex` (for execution, marked `#[serde(skip)]`). Implement custom `Deserialize` that recompiles the regex from the stored pattern. Construction validates the regex eagerly (FR-010).

**Rationale**: Clean separation of serialization concern from runtime. Deserialization failure on invalid regex is caught at load time, consistent with construction-time validation.

**Alternatives considered**:
- Serialize only pattern, recompile on every match (performance penalty — rejected)
- Use `serde_regex` crate (tiny dependency for one field, prefer hand-rolled — rejected)

### R5: Cancellation Propagation in Parallel Pipelines

**Question**: How do First/Fastest strategies cancel remaining branches?

**Finding**: The existing project uses `tokio_util::CancellationToken` with child tokens. Each branch gets a child of the pipeline's token. For First/Fastest, the executor creates a shared child token for all branches, then cancels it when enough results arrive. Branches check `token.is_cancelled()` during agent execution (the agent loop already respects cancellation tokens).

**Decision**: Use `CancellationToken::child_token()` per branch. The executor monitors a `tokio::sync::mpsc` channel for completed branches. When the merge strategy's completion criteria is met, cancel the shared branch token. This cancels all remaining branches' agent loops.

**Rationale**: Matches existing cancellation patterns in the codebase. Zero new dependencies. Agent loops already respect `CancellationToken`.

### R6: AgentFactory Convenience Patterns

**Question**: How should common use cases be handled without requiring consumers to implement `AgentFactory` from scratch?

**Finding**: Most consumers will want to register named agent configurations and have the executor create fresh instances by name. The factory pattern should support this without exposing raw closure boilerplate.

**Decision**: Provide `SimpleAgentFactory` — a concrete struct that holds `HashMap<String, Arc<dyn Fn() -> AgentOptions + Send + Sync>>`. Methods:
- `new()` — empty factory
- `register(name, factory_fn)` — register a named agent creator
- Implements `AgentFactory` — calls the registered factory fn, constructs `Agent::new(options)`

Also provide `impl AgentFactory for Arc<dyn Fn(&str) -> Result<Agent, PipelineError> + Send + Sync>` for maximum flexibility.

**Rationale**: Two levels of convenience — struct for common case, bare closure for advanced. No dependency on `AgentRegistry` (the factory is independent).
