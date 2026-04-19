# Contract: Plugin Trait

## Public API Surface

```rust
/// A named, prioritized bundle of policies, tools, and event observers.
///
/// All methods except `name()` have default no-op implementations.
/// Plugins are registered via `AgentOptions::with_plugin()` and their
/// contributions are merged into the agent at construction time.
pub trait Plugin: Send + Sync {
    /// Unique identifier. Used for registry lookup and tool namespacing.
    fn name(&self) -> &str;

    /// Execution priority. Higher values run first. Default: 0.
    fn priority(&self) -> i32 { 0 }

    /// Called once during Agent::new(), after full configuration,
    /// before the first conversation turn. Receives read-only agent ref.
    /// Panics are caught and logged; agent construction continues.
    fn on_init(&self, _agent: &Agent) {}

    /// Policies contributed to the pre-turn slot.
    fn pre_turn_policies(&self) -> Vec<Arc<dyn PreTurnPolicy>> { vec![] }

    /// Policies contributed to the pre-dispatch slot.
    fn pre_dispatch_policies(&self) -> Vec<Arc<dyn PreDispatchPolicy>> { vec![] }

    /// Policies contributed to the post-turn slot.
    fn post_turn_policies(&self) -> Vec<Arc<dyn PostTurnPolicy>> { vec![] }

    /// Policies contributed to the post-loop slot.
    fn post_loop_policies(&self) -> Vec<Arc<dyn PostLoopPolicy>> { vec![] }

    /// Event observer. Called for every AgentEvent in dispatch order.
    /// Panics are caught and logged (consistent with event forwarders).
    fn on_event(&self, _event: &AgentEvent) {}

    /// Tools contributed to the agent. Auto-namespaced as "{plugin_name}_{tool_name}".
    /// Both components are sanitized to match the strictest provider tool-name
    /// grammar (`^[a-zA-Z][a-zA-Z0-9_]{0,63}$`).
    fn tools(&self) -> Vec<Arc<dyn AgentTool>> { vec![] }
}
```

## Registry API

```rust
pub struct PluginRegistry { /* ... */ }

impl PluginRegistry {
    pub fn new() -> Self;
    pub fn register(&mut self, plugin: Arc<dyn Plugin>);
    pub fn unregister(&mut self, name: &str);
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Plugin>>;
    pub fn list(&self) -> &[Arc<dyn Plugin>];  // sorted by priority (desc)
    pub fn is_empty(&self) -> bool;
    pub fn len(&self) -> usize;
}
```

## AgentOptions Extension

```rust
impl AgentOptions {
    pub fn with_plugin(self, plugin: Arc<dyn Plugin>) -> Self;
    pub fn with_plugins(self, plugins: Vec<Arc<dyn Plugin>>) -> Self;
}
```

## Agent Extension

```rust
impl Agent {
    pub fn plugins(&self) -> &[Arc<dyn Plugin>];
    pub fn plugin(&self, name: &str) -> Option<&Arc<dyn Plugin>>;
}
```

## Invariants

1. Plugin names are unique within a registry. Duplicate registration replaces + warns.
2. Plugin contributions are extracted once at `Agent::new()` and merged into existing vecs.
3. Plugin policies are prepended (run before direct policies). Plugin tools are appended (direct tools take precedence on name collision).
4. Plugin event observers are prepended to forwarder list (run before direct forwarders).
5. `on_init` fires in priority order (highest first) with `catch_unwind`.
6. After `Agent::new()`, plugins are retained for introspection only — their contributions live in the merged vecs.
