# Implementation Plan: Multi-Agent Patterns Crate & Pipeline Primitives

**Branch**: `039-multi-agent-patterns` | **Date**: 2026-04-02 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/039-multi-agent-patterns/spec.md`

## Summary

New `swink-agent-patterns` workspace crate at `patterns/` providing composable multi-agent pipeline primitives — Sequential, Parallel, and Loop — with a registry, stateless executor, event emission, and tool-system bridge. Follows the `swink-agent-policies` crate pattern: separate crate, depends only on `swink-agent` public API, feature-gated modules.

## Technical Context

**Language/Version**: Rust 1.88 (edition 2024)  
**Primary Dependencies**: `swink-agent` (path = ".."), `tokio` (async runtime), `tokio-util` (CancellationToken), `serde`/`serde_json` (serialization), `regex` (exit conditions), `uuid` (PipelineId generation), `tracing` (diagnostics), `thiserror` (error types)  
**Storage**: N/A (in-memory registries only)  
**Testing**: `cargo test -p swink-agent-patterns`  
**Target Platform**: Cross-platform library  
**Project Type**: Library crate (workspace member)  
**Performance Goals**: Zero overhead when event handler absent; parallel branches achieve true concurrency  
**Constraints**: Depends only on `swink-agent` public API — no internal imports  
**Scale/Scope**: ~7 source files in `src/pipeline/` module, ~2000 LOC estimated

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | PASS | New workspace crate, independently compilable and testable. Justification for new crate: pipeline patterns are a cross-cutting composition concern that doesn't belong in core (which must remain provider/pattern-agnostic) or in any existing crate. |
| II. Test-Driven Development | PASS | Tests written before implementation. Shared test helpers for mock agents. |
| III. Efficiency & Performance | PASS | `tokio::spawn` for parallel branches. Event emission is no-op when handler absent. |
| IV. Leverage the Ecosystem | PASS | Uses `regex`, `uuid`, `tokio`, `serde` — all existing workspace deps. No new external deps. |
| V. Provider Agnosticism | PASS | Pipeline system is provider-agnostic. Uses `AgentFactory` trait — no provider-specific types. |
| VI. Safety & Correctness | PASS | `#[forbid(unsafe_code)]`. Panics caught at task boundaries for parallel branches. `max_iterations` enforced — no unbounded loops. |

**Architectural Constraints**:
- **Crate count**: Currently 7 members → 8 with `patterns`. Justified: pipeline composition patterns don't fit in core (FR-001), adapters (provider-specific), policies (evaluation hooks), memory (persistence), eval (testing), TUI (frontend), or xtask (tooling). Patterns is a new concern boundary.
- **MSRV**: 1.88, edition 2024.
- **Events**: Pipeline events use `Emission` via `AgentEvent::Custom` — no core enum modification needed.

## Project Structure

### Documentation (this feature)

```text
specs/039-multi-agent-patterns/
├── spec.md              # Feature specification
├── plan.md              # This file
├── research.md          # Phase 0: design decisions
├── data-model.md        # Phase 1: entity definitions
├── quickstart.md        # Phase 1: usage examples
├── contracts/
│   └── public-api.md    # Phase 1: API surface contract
├── checklists/
│   └── requirements.md  # Spec quality checklist
└── tasks.md             # Phase 2 output (via /speckit.tasks)
```

### Source Code (repository root)

```text
patterns/
├── Cargo.toml
└── src/
    ├── lib.rs               # #![forbid(unsafe_code)], feature-gated re-exports
    └── pipeline/
        ├── mod.rs           # Module re-exports
        ├── types.rs         # Pipeline, PipelineId, MergeStrategy, ExitCondition
        ├── registry.rs      # PipelineRegistry (RwLock<HashMap>)
        ├── executor.rs      # PipelineExecutor, AgentFactory trait, SimpleAgentFactory
        ├── output.rs        # PipelineOutput, StepResult, PipelineError
        ├── events.rs        # PipelineEvent enum, to_emission()
        └── tool.rs          # PipelineTool (AgentTool impl)
```

**Structure Decision**: Single `pipeline/` module directory with one file per concern. This mirrors `policies/src/` but uses a directory instead of flat files because the pipeline module has 7 distinct subcomponents that share types internally. The `lib.rs` re-exports everything behind `#[cfg(feature = "pipelines")]`.

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| 8th workspace crate | Pipeline composition patterns don't fit any existing boundary. Core must remain pattern-agnostic (Constitution I). Policies are evaluation hooks, not execution orchestration. | Putting pipelines in core violates Library-First (core stays lean). Putting them in an existing crate creates coupling. |
| AgentFactory trait | `Agent` and `AgentOptions` are not `Clone` due to `Box<dyn Fn>` closure fields. Executor needs fresh agent instances per step. | Direct `AgentRegistry` use requires locking and cloning config — impossible without modifying core types. Factory decouples executor from instantiation strategy. |

## Key Design Decisions

### 1. AgentFactory instead of AgentRegistry coupling

The executor accepts `Arc<dyn AgentFactory>` rather than `Arc<AgentRegistry>`. This is necessary because `Agent`/`AgentOptions` are not `Clone` (closure fields use `Box`). The factory trait lets consumers control agent instantiation. `SimpleAgentFactory` provides a convenient `HashMap<name → factory_fn>` implementation for common use cases. See [research.md](research.md) R1.

### 2. PipelineEvent callback instead of direct Agent event dispatch

The executor accepts `Option<Arc<dyn Fn(PipelineEvent) + Send + Sync>>`. Pipeline events are a crate-local enum with `to_emission()` for bridging to `AgentEvent::Custom`. This avoids coupling the patterns crate to core event dispatch internals. See [research.md](research.md) R2.

### 3. ExitCondition regex serialization

`ExitCondition::OutputContains` stores both the pattern string (serializable) and compiled `regex::Regex` (`#[serde(skip)]`). Custom `Deserialize` recompiles on load. See [research.md](research.md) R4.

### 4. Pipeline carries its own ID

Pipelines embed their `PipelineId` at construction (auto-UUID if not provided). The registry uses this ID as its key — `register(pipeline)` reads the ID, it doesn't assign one. This keeps pipelines self-describing and serializable.

### 5. Fresh agent instances for all pipeline types

All pipeline execution (sequential, parallel, loop) creates fresh `Agent` instances via the `AgentFactory`. Registered agents serve as templates. This makes execution stateless, repeatable, and free of mutex contention for parallel branches.
