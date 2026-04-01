# Data Model: Plugin System

**Feature**: 037-plugin-system  
**Date**: 2026-04-01

## Entities

### Plugin (trait)

The core abstraction. A named, prioritized bundle of contributions.

| Method | Return Type | Default | Description |
|--------|------------|---------|-------------|
| `name()` | `&str` | (required) | Unique identifier for the plugin |
| `priority()` | `i32` | `0` | Execution order — higher runs first |
| `on_init()` | `()` | no-op | Called once during agent construction |
| `pre_turn_policies()` | `Vec<Arc<dyn PreTurnPolicy>>` | `vec![]` | Policies for pre-turn slot |
| `pre_dispatch_policies()` | `Vec<Arc<dyn PreDispatchPolicy>>` | `vec![]` | Policies for pre-dispatch slot |
| `post_turn_policies()` | `Vec<Arc<dyn PostTurnPolicy>>` | `vec![]` | Policies for post-turn slot |
| `post_loop_policies()` | `Vec<Arc<dyn PostLoopPolicy>>` | `vec![]` | Policies for post-loop slot |
| `on_event()` | `()` | no-op | Event observer, called for every AgentEvent |
| `tools()` | `Vec<Arc<dyn AgentTool>>` | `vec![]` | Tools contributed to the agent |

**Constraints**:
- `Send + Sync` required (consistent with all agent traits)
- All methods take `&self` (sync, non-mutating)
- `name()` is the only required method — all others have defaults

### PluginRegistry

Configuration-time container for plugins.

| Field | Type | Description |
|-------|------|-------------|
| `plugins` | `Vec<Arc<dyn Plugin>>` | Registered plugins, maintained in insertion order |

| Operation | Behavior |
|-----------|----------|
| `register(plugin)` | Add or replace (by name). Warn on duplicate. |
| `unregister(name)` | Remove by name. No-op if not found. |
| `get(name)` | Lookup by name. Returns `Option<&Arc<dyn Plugin>>`. |
| `list()` | All plugins in priority-sorted order (stable sort, descending). |
| `is_empty()` | True if no plugins registered. |

### NamespacedTool (internal wrapper)

Transparent wrapper that prefixes a plugin's name onto a tool's name.

| Field | Type | Description |
|-------|------|-------------|
| `plugin_name` | `String` | The owning plugin's name |
| `inner` | `Arc<dyn AgentTool>` | The original tool |

**Behavior**: Delegates all `AgentTool` methods to `inner` except `name()`, which returns `"{plugin_name}.{inner.name()}"`.

## Relationships

```
AgentOptions
  └── plugins: Vec<Arc<dyn Plugin>>
        │
        ▼ (consumed in Agent::new())
Agent
  ├── plugins: Vec<Arc<dyn Plugin>>      ← retained for introspection
  ├── pre_turn_policies: Vec<Arc<...>>   ← plugin policies prepended
  ├── pre_dispatch_policies: Vec<Arc<...>>
  ├── post_turn_policies: Vec<Arc<...>>
  ├── post_loop_policies: Vec<Arc<...>>
  ├── tools: Vec<Arc<dyn AgentTool>>     ← namespaced plugin tools appended
  └── event_forwarders: Vec<...>         ← plugin observers prepended
```

## Merge Order (per slot)

```
[Plugin A policies (priority 10)]
[Plugin B policies (priority 5)]
[Plugin C policies (priority 0)]
[Directly-registered policies]
```

## Tool Merge Order

```
[Directly-registered tools]              ← checked first (take precedence)
[Plugin A tools (priority 10), namespaced as "pluginA.toolName"]
[Plugin B tools (priority 5), namespaced as "pluginB.toolName"]
```
