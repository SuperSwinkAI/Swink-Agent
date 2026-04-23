# Implementation Plan: Plugin System

**Branch**: `037-plugin-system` | **Date**: 2026-04-01 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/037-plugin-system/spec.md`

## Summary

Add a named plugin abstraction that bundles policies, tools, and event observers into a single registerable unit with priority-based execution ordering. Plugin contributions are merged with directly-registered policies and tools at agent construction time. The plugin trait and registry live in the core `swink-agent` crate behind a `plugins` feature gate. No new workspace crate required.

## Technical Context

**Language/Version**: Rust latest stable, edition 2024  
**Primary Dependencies**: `swink-agent` core types (policy traits, AgentTool, AgentEvent), `tracing` (diagnostics)  
**Storage**: N/A (in-memory registry only)  
**Testing**: `cargo test -p swink-agent`  
**Target Platform**: All (library crate)  
**Project Type**: Library  
**Performance Goals**: Zero overhead when no plugins registered; single-pass merge at construction time  
**Constraints**: Feature-gated (`plugins`), no new external deps, no unsafe  
**Scale/Scope**: Trait + registry (~300 LOC), integration with AgentOptions/Agent (~100 LOC), tests (~400 LOC)

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | PASS | Plugin trait in core crate. No new crate — registry is small enough for core. Feature-gated to avoid polluting default builds. |
| II. Test-Driven | PASS | Tests before implementation. Unit tests for registry operations, integration tests for policy/tool merging. |
| III. Efficiency & Performance | PASS | Merge happens once at construction. No per-turn overhead. Empty plugin vec = zero cost. |
| IV. Leverage the Ecosystem | PASS | No external deps needed. Uses existing `Arc`, `catch_unwind`, `tracing`. |
| V. Provider Agnosticism | PASS | Plugins are provider-agnostic — operate on core types only. |
| VI. Safety & Correctness | PASS | `catch_unwind` for init/observer panics. No unsafe. Poisoned lock recovery N/A (no locks in registry). |

**Architectural Constraints:**
- Crate count: No new crate. Plugin trait + registry added to core behind feature gate.
- Events are outward-only: Plugin event observers are read-only (`&AgentEvent`), consistent with existing forwarder pattern.
- No global mutable state: Registry owned by AgentOptions, then Agent. No statics.

No violations. No complexity tracking needed.

## Project Structure

### Documentation (this feature)

```text
specs/037-plugin-system/
├── spec.md
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
└── tasks.md             # Phase 2 output (via /speckit.tasks)
```

### Source Code (repository root)

```text
src/
├── plugin.rs            # NEW: Plugin trait, PluginRegistry, merge logic
├── agent_options.rs     # MODIFIED: with_plugin(), with_plugins() builders
├── agent.rs             # MODIFIED: plugin init callbacks in new()
├── agent/
│   ├── events.rs        # MODIFIED: plugin observers before forwarders
│   └── invoke.rs        # MODIFIED: merged policies/tools into loop config
├── loop_/
│   └── config.rs        # NO CHANGE: receives merged vecs as before
├── policy.rs            # NO CHANGE: run_policies() unchanged
├── lib.rs               # MODIFIED: conditional re-export of plugin module

tests/
├── plugin_registry.rs   # NEW: unit tests for registry CRUD, ordering
├── plugin_integration.rs # NEW: end-to-end plugin policy/tool/event tests
└── common/mod.rs        # MODIFIED: MockPlugin helper
```

**Structure Decision**: All plugin code lives in core crate under `src/plugin.rs`. The module is feature-gated with `#[cfg(feature = "plugins")]`. This avoids a new crate (satisfies constitution) while keeping the feature opt-in. The existing policy slot runners (`policy.rs`) and tool dispatch (`tool_dispatch.rs`) are unchanged — plugins contribute to the existing `Vec<Arc<dyn Policy>>` and `Vec<Arc<dyn AgentTool>>` collections, which are merged in `AgentOptions` or `Agent::new()`.

## Design Decisions

### D1: Merge Point — AgentOptions vs Agent::new()

**Decision**: Merge plugin contributions in `Agent::new()`, not in `AgentOptions` builder methods.

**Rationale**: `AgentOptions` is a plain data bag. Merging in `new()` allows the registry to be frozen and contributions extracted in one pass. It also ensures `on_init` fires after the agent is fully constructed.

**Flow**:
1. `AgentOptions` stores `plugins: Vec<Arc<dyn Plugin>>` alongside existing policy/tool vecs
2. `Agent::new()` sorts plugins by priority, extracts contributions, prepends to policy vecs
3. `Agent::new()` calls `on_init(&self)` for each plugin (priority order, with catch_unwind)
4. `Agent::new()` wraps plugin `on_event` as event forwarders, prepended before direct forwarders

### D2: Tool Namespacing

**Decision**: Plugin tools are namespace-prefixed as `{plugin_name}.{tool_name}`.

**Rationale**: Prevents collisions between plugins. Direct tools are not namespaced. If a namespaced plugin tool collides with a direct tool name (unlikely), the direct tool wins.

**Implementation**: When extracting tools from a plugin, wrap each `AgentTool` in a `NamespacedTool` newtype that delegates all trait methods but overrides `name()` to return `"{plugin_name}.{tool_name}"`.

### D3: Event Observer Integration

**Decision**: Plugin `on_event(&AgentEvent)` calls are converted to `EventForwarderFn` closures at construction and prepended to the forwarder list.

**Rationale**: Reuses the existing forwarder dispatch path in `agent/events.rs` (which already has catch_unwind). No new dispatch mechanism needed. Plugin observers run first because they're prepended.

### D4: Priority Sorting Stability

**Decision**: Use stable sort on plugins by priority (descending). Insertion order preserved as tiebreaker.

**Rationale**: `Vec::sort_by` in Rust is stable. Sorting once at construction time. Higher priority = runs first (consistent with middleware patterns).
