# Research: Plugin System

**Feature**: 037-plugin-system  
**Date**: 2026-04-01

## R1: Plugin Trait Design — Sync vs Async Methods

**Decision**: All Plugin trait methods are synchronous (`&self`).

**Rationale**: Plugin contributions (policies, tools) are collected once at construction time. `on_init` and `on_event` are synchronous observers consistent with existing patterns (event forwarders are `Fn(AgentEvent)`, not async). Plugins that need async work should spawn tasks internally via `tokio::spawn`.

**Alternatives considered**:
- Async trait methods: Would require `async_trait` or RPITIT. Adds complexity for a construction-time operation. Rejected — async init is uncommon and can be worked around.
- Separate async init phase: Would require splitting agent construction into build + init steps. Rejected — breaks the current `Agent::new()` single-step pattern.

## R2: NamespacedTool Wrapper

**Decision**: Implement a `NamespacedTool` newtype struct that wraps `Arc<dyn AgentTool>` and overrides `name()` to return `"{plugin_name}.{tool_name}"`.

**Rationale**: Clean delegation pattern. The wrapper implements `AgentTool` by forwarding all methods to the inner tool except `name()`. This avoids modifying the Plugin trait to return pre-namespaced tools (which would leak the namespacing responsibility to plugin authors).

**Alternatives considered**:
- Require plugin authors to namespace their own tools: Burdensome, error-prone, inconsistent naming. Rejected.
- Use a HashMap keyed by `(plugin_name, tool_name)` tuples: Breaks the existing `Vec<Arc<dyn AgentTool>>` tool collection pattern. Rejected.

## R3: Plugin Registry Ownership

**Decision**: `PluginRegistry` is a simple `Vec<Arc<dyn Plugin>>` wrapper stored on `AgentOptions`. It is consumed during `Agent::new()` — contributions are extracted and merged into the agent's policy/tool/forwarder collections.

**Rationale**: The registry exists only during configuration. After `Agent::new()`, plugins are "dissolved" into the agent's existing collections. No need to keep the registry alive at runtime — introspection is available via the stored `Vec<Arc<dyn Plugin>>` reference on the Agent struct.

**Alternatives considered**:
- Keep registry as a runtime component on Agent: Adds complexity. The agent already stores its own policy/tool vecs. Keeping a parallel registry creates two sources of truth. Rejected in favor of "dissolve at construction" with a retained plugin list for introspection only.

## R4: Integration with Existing Policy Slot Runners

**Decision**: No changes to `run_policies()`, `run_post_turn_policies()`, `run_post_loop_policies()`, or `run_pre_dispatch_policies()` in `policy.rs`.

**Rationale**: These functions operate on `&[Arc<dyn Policy>]` slices. Plugin policies are merged into the existing policy vecs before the loop starts. The slot runners don't need to know about plugins — they just evaluate whatever policies are in the vec.

**Alternatives considered**:
- Modify slot runners to accept a plugin registry: Would couple the policy evaluation logic to the plugin system. Rejected — separation of concerns.

## R5: Duplicate Plugin Name Handling

**Decision**: Log a warning via `tracing::warn!` and replace the existing plugin with the new one.

**Rationale**: Consistent with last-writer-wins semantics. The consumer is warned but not blocked. This matches the existing pattern for tool name collisions (direct tools silently override).

**Alternatives considered**:
- Return an error on duplicate name: Too strict for a configuration-time operation. Would require error handling in builder chains.
- Silently ignore the duplicate: Could mask configuration bugs. Warning is the right balance.
