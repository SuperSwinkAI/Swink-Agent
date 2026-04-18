# Quickstart: Plugin System

**Feature**: 037-plugin-system  
**Date**: 2026-04-01

## Creating a Plugin

Implement the `Plugin` trait. Only `name()` is required — all other methods have defaults.

```rust
use swink_agent::{Plugin, AgentEvent};
use swink_agent::policy::{PreTurnPolicy, PolicyContext, PolicyVerdict};
use std::sync::Arc;

struct MyAuditPlugin;

impl Plugin for MyAuditPlugin {
    fn name(&self) -> &str { "audit" }

    fn priority(&self) -> i32 { 10 }  // runs before default (0)

    fn post_turn_policies(&self) -> Vec<Arc<dyn PostTurnPolicy>> {
        vec![Arc::new(MyAuditPolicy)]
    }

    fn on_event(&self, event: &AgentEvent) {
        // Log every event for audit trail
        tracing::info!(plugin = "audit", ?event);
    }
}
```

## Registering Plugins

Add plugins via the agent options builder:

```rust
let agent = Agent::new(
    AgentOptions::new(stream_fn)
        .with_plugin(Arc::new(MyAuditPlugin))
        .with_plugin(Arc::new(SecurityPlugin::new(config)))
        .with_pre_turn_policy(Arc::new(my_budget_policy))  // still works
);
```

## Plugin with Tools

Plugins can contribute tools. Tools are auto-namespaced as `{plugin_name}_{tool_name}` (both components sanitized to `^[a-zA-Z][a-zA-Z0-9_]{0,63}$` so the result is accepted by every supported provider):

```rust
impl Plugin for ArtifactPlugin {
    fn name(&self) -> &str { "artifacts" }

    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        vec![
            Arc::new(SaveArtifactTool::new(self.store.clone())),  // → "artifacts_save_artifact"
            Arc::new(LoadArtifactTool::new(self.store.clone())),  // → "artifacts_load_artifact"
        ]
    }
}
```

## Priority Ordering

Higher priority runs first. Same priority uses insertion order.

```rust
// Security (priority 100) runs before audit (priority 10) runs before default (0)
let agent = Agent::new(
    AgentOptions::new(stream_fn)
        .with_plugin(Arc::new(AuditPlugin))       // priority 10
        .with_plugin(Arc::new(SecurityPlugin))     // priority 100
        // SecurityPlugin's policies evaluate first despite being registered second
);
```

## Inspecting Registered Plugins

```rust
let agent = Agent::new(options);

for plugin in agent.plugins() {
    println!("{} (priority {})", plugin.name(), plugin.priority());
}

if let Some(audit) = agent.plugin("audit") {
    println!("Audit plugin is active");
}
```

## Feature Gate

Enable the `plugins` feature in your `Cargo.toml`:

```toml
[dependencies]
swink-agent = { version = "0.4", features = ["plugins"] }
```
