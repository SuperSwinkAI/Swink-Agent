# Feature Specification: Plugin System

**Feature Branch**: `037-plugin-system`  
**Created**: 2026-04-01  
**Status**: Draft  
**Input**: User description: "Plugin System — named plugin bundles with priority ordering that compose policies, tools, and event observers into reusable units"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Consumer Adds a Plugin That Bundles Policies and Tools (Priority: P1)

A library consumer building an agent-powered application wants to add audit logging. Today they must independently register an audit policy to the post-turn slot, configure an audit sink, and wire up an event forwarder — three separate registration calls that must be kept in sync. With the plugin system, they add a single audit plugin that bundles all of these together. The plugin contributes its policies to the correct slots, registers its event observer, and contributes any tools it provides — all from a single registration call.

**Why this priority**: Bundling related concerns into a single registerable unit is the core value proposition. Without it, there is no plugin system — only independent policy/tool registration (which already exists).

**Independent Test**: Can be fully tested by creating a plugin that contributes one policy and one tool, registering it on an agent, and verifying both the policy fires during the loop and the tool appears in the tool list.

**Acceptance Scenarios**:

1. **Given** an agent configured with a plugin that contributes a post-turn policy and a tool, **When** the agent runs a conversation, **Then** the post-turn policy evaluates at the end of each turn and the tool is available to the LLM.
2. **Given** a plugin that contributes policies to multiple slots (pre-turn and post-turn), **When** the plugin is registered, **Then** policies appear in the correct slots without the consumer needing to register them individually.
3. **Given** a plugin that observes events, **When** agent events fire during the loop, **Then** the plugin's event observer is called for each event.

---

### User Story 2 - Consumer Controls Plugin Execution Order via Priority (Priority: P1)

A library consumer registers multiple plugins — a security plugin (prompt injection guard) and a logging plugin. The security plugin must run before the logging plugin so that rejected requests are never logged as successful. The consumer assigns priorities to control execution order: higher priority runs first.

**Why this priority**: Without ordering guarantees, plugins that depend on evaluation order (security before logging, validation before transformation) cannot be composed safely. Priority is the mechanism that makes multi-plugin compositions predictable.

**Independent Test**: Can be fully tested by registering two plugins with different priorities that contribute policies to the same slot, and verifying the higher-priority plugin's policy evaluates first.

**Acceptance Scenarios**:

1. **Given** plugin A (priority 10) and plugin B (priority 5) both contribute pre-turn policies, **When** the pre-turn slot evaluates, **Then** plugin A's policy runs before plugin B's policy.
2. **Given** two plugins with the same priority (both 0), **When** they are registered in order [X, Y], **Then** X's policies run before Y's policies (insertion order breaks ties).
3. **Given** plugin A (priority 10) contributes a pre-turn policy that returns Stop, **When** the pre-turn slot evaluates, **Then** plugin B's pre-turn policy (priority 5) is never evaluated (short-circuit).

---

### User Story 3 - Plugins Compose with Directly-Registered Policies (Priority: P1)

A library consumer has an existing agent setup with directly-registered policies (e.g., a custom budget policy added via the existing policy slot API). They add a plugin on top without removing their direct registrations. Plugin-contributed policies run before directly-registered policies, but both are evaluated. The consumer's existing setup continues to work unchanged.

**Why this priority**: Backward compatibility is non-negotiable. The plugin system must layer on top of the existing policy system, not replace it. Consumers must be able to adopt plugins incrementally.

**Independent Test**: Can be fully tested by configuring an agent with both a directly-registered pre-turn policy and a plugin that contributes a pre-turn policy, then verifying both are evaluated in the correct order.

**Acceptance Scenarios**:

1. **Given** an agent with a directly-registered pre-turn policy and a plugin that contributes a pre-turn policy, **When** the pre-turn slot evaluates, **Then** the plugin's policy runs first, followed by the directly-registered policy.
2. **Given** an agent with directly-registered policies and no plugins, **When** the agent runs, **Then** behavior is identical to the current system (zero behavioral change).
3. **Given** a plugin's pre-turn policy returns Stop, **When** the pre-turn slot evaluates, **Then** the directly-registered policies are not evaluated (short-circuit applies across the merged policy list).

---

### User Story 4 - Consumer Discovers and Inspects Registered Plugins (Priority: P2)

A library consumer building a debugging or admin interface wants to list all registered plugins, inspect their names, priorities, and what they contribute (which policy slots, which tools). The plugin registry provides introspection capabilities so the consumer can display the active plugin configuration.

**Why this priority**: Introspection is important for debuggability and operational visibility but is not required for plugins to function. It supports tooling and diagnostics built on top of the plugin system.

**Independent Test**: Can be fully tested by registering several plugins and calling registry introspection methods to verify names, priorities, and contribution summaries are correct.

**Acceptance Scenarios**:

1. **Given** three registered plugins ("audit", "security", "telemetry"), **When** the consumer queries the registry, **Then** all three are listed in priority order with their names.
2. **Given** a registered plugin named "audit", **When** the consumer queries for plugin "audit", **Then** the plugin reference is returned.
3. **Given** no plugins registered, **When** the consumer queries the registry, **Then** an empty list is returned.

---

### User Story 5 - Plugin Receives Initialization Callback (Priority: P2)

A library consumer writes a plugin that needs to perform setup when it is attached to an agent — for example, validating that required tools exist, checking configuration, or logging activation. The plugin receives an initialization callback after the agent is fully constructed, giving it read access to the agent's configuration.

**Why this priority**: Initialization enables plugins to validate their environment and perform one-time setup. However, most simple plugins (policy bundles, event observers) don't need initialization, making this an enhancement rather than a core requirement.

**Independent Test**: Can be fully tested by creating a plugin with an initialization callback that records whether it was called, registering it, building the agent, and verifying the callback fired.

**Acceptance Scenarios**:

1. **Given** a plugin with an initialization callback, **When** the agent is constructed, **Then** the callback is called once with a reference to the agent.
2. **Given** multiple plugins with initialization callbacks, **When** the agent is constructed, **Then** callbacks fire in priority order (highest first).
3. **Given** a plugin with no initialization callback (default no-op), **When** the agent is constructed, **Then** no error occurs and no callback fires for that plugin.

---

### User Story 6 - Consumer Removes a Plugin at Configuration Time (Priority: P3)

A library consumer wants to conditionally exclude a plugin based on runtime configuration — for example, disabling the telemetry plugin in a development environment. They can unregister a plugin by name before the agent starts, and all of its contributed policies, tools, and event observers are removed.

**Why this priority**: Plugin removal is a configuration convenience. Most consumers will simply not register plugins they don't want. Removal is useful for dynamic configuration scenarios but is not required for the core plugin workflow.

**Independent Test**: Can be fully tested by registering a plugin, unregistering it by name, and verifying its policies and tools no longer appear in the agent's configuration.

**Acceptance Scenarios**:

1. **Given** a registered plugin named "telemetry", **When** the consumer unregisters "telemetry", **Then** all policies and tools contributed by that plugin are removed.
2. **Given** no plugin named "nonexistent", **When** the consumer unregisters "nonexistent", **Then** the operation succeeds silently (idempotent).

---

### User Story 7 - Plugin Contributes Tools to the Agent (Priority: P2)

A library consumer uses a plugin that provides specialized tools — for example, an MCP plugin that contributes tools dynamically fetched from an MCP server, or an artifact plugin that provides save/load/list artifact tools. The plugin's tools are merged with the agent's directly-registered tools and are available to the LLM.

**Why this priority**: Tool contribution is a key plugin capability that enables self-contained feature bundles (e.g., an artifact plugin contributes both storage policies and artifact tools). However, tools can also be registered directly, so this is an enhancement to the bundling story.

**Independent Test**: Can be fully tested by creating a plugin that contributes two tools, registering it, and verifying both tools appear in the agent's tool list and are callable by the LLM.

**Acceptance Scenarios**:

1. **Given** a plugin named "artifacts" contributing tools "save" and "load", **When** the plugin is registered, **Then** tools "artifacts.save" and "artifacts.load" appear in the agent's tool list alongside any directly-registered tools.
2. **Given** two plugins "alpha" and "beta" both contributing a tool named "export", **When** both are registered, **Then** "alpha.export" and "beta.export" coexist without collision.
3. **Given** a plugin contributing tools and a plugin contributing policies, **When** both are registered, **Then** both plugins' contributions are independently active.

---

### Edge Cases

- What happens when two plugins have the same name? The second registration replaces the first. Plugin names must be unique within a registry. The consumer is warned via a log message.
- What happens when a plugin's initialization callback panics? The panic is caught (consistent with the existing `catch_unwind` pattern for event forwarders). The plugin is logged as failed but the agent continues construction. The plugin's policies and tools are still registered — only the init callback failed.
- What happens when a plugin contributes a policy that panics during evaluation? Handled by the existing policy slot panic safety (`catch_unwind` + `AssertUnwindSafe`). The plugin's policy is treated like any other panicking policy.
- What happens when a plugin contributes zero policies, zero tools, and no event observer? The plugin is registered successfully as a no-op. This is valid — a plugin may exist solely for its initialization callback or as a future extension point.
- What happens when plugins are registered after the agent has started? Plugins can only be registered during agent construction (via the builder/options). There is no runtime plugin addition — this avoids the complexity of hot-swapping policies and tools mid-conversation.
- What happens when a plugin's event observer is slow? Event observers run synchronously in the event dispatch path (consistent with existing forwarder behavior). A slow observer blocks event delivery. Plugins that need async observation should spawn their own tasks internally.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST provide a Plugin abstraction that bundles a name, priority, lifecycle callbacks, policy contributions (for all four slots), event observation, and tool contributions into a single registerable unit.
- **FR-002**: All Plugin capabilities MUST be opt-in with default no-op implementations. A minimal plugin needs only a name.
- **FR-003**: System MUST provide a Plugin Registry that maintains a collection of registered plugins sorted by priority (highest first), with insertion order as tiebreaker.
- **FR-004**: Plugin registration MUST enforce unique names. Registering a plugin with an existing name MUST replace the previous plugin.
- **FR-005**: Plugin unregistration by name MUST remove the plugin and all of its contributions. Unregistering a nonexistent name MUST succeed silently.
- **FR-006**: The registry MUST support lookup by name and listing all registered plugins.
- **FR-007**: Plugin-contributed policies MUST be merged with directly-registered policies at each slot. Plugin policies MUST run before directly-registered policies. Among plugin-contributed policies, higher-priority plugins' policies MUST run first.
- **FR-008**: Short-circuit semantics (Stop verdict) MUST apply across the merged policy list — a Stop from a plugin policy prevents evaluation of subsequent plugin policies and all directly-registered policies in that slot.
- **FR-009**: Plugin-contributed tools MUST be namespace-prefixed as `{plugin_name}_{tool_name}` (e.g., a tool "save" from plugin "artifacts" becomes "artifacts_save"). Both components are sanitized to the common subset accepted by every supported provider's tool-name grammar (`^[a-zA-Z][a-zA-Z0-9_]{0,63}$`). This prevents name collisions between plugins and guarantees the composed name is accepted by Anthropic, OpenAI, Bedrock, Mistral, Gemini, Ollama, and Azure. Directly-registered tools are not namespaced. If a namespace-prefixed plugin tool collides with a directly-registered tool name, the directly-registered tool MUST take precedence. *(Separator was changed from `.` to `_` in response to issue #608; dots are rejected by Anthropic and Bedrock.)*
- **FR-010**: Plugin event observers MUST be called for every agent event, in plugin priority order (highest first), before any directly-registered event forwarders.
- **FR-011**: Plugin initialization callbacks MUST be called once during agent construction, in priority order, after the agent is fully configured but before the first conversation turn.
- **FR-012**: Panics in plugin initialization callbacks MUST be caught and logged. The agent MUST continue construction. The plugin's other contributions (policies, tools, observers) MUST remain active.
- **FR-013**: Plugin registration MUST be supported via the existing agent configuration builder (e.g., `with_plugin`, `with_plugins`). The plugin system MUST be additive — existing configuration patterns without plugins MUST continue to work identically.
- **FR-014**: Plugins MUST only be registerable during agent construction. There is no runtime plugin addition or removal once the agent loop has started.
- **FR-015**: The plugin system MUST be feature-gated so that projects not using plugins incur no compile-time or runtime cost.
- **FR-016**: Plugin event observers MUST be protected against panics using the same `catch_unwind` pattern used for existing event forwarders.

### Key Entities

- **Plugin**: The bundling abstraction — a named, prioritized unit that contributes policies, tools, event observers, and lifecycle callbacks. The core concept of the system.
- **PluginRegistry**: The container that manages registered plugins — maintains sort order, enforces unique names, and provides lookup/listing. Owned by the agent configuration.
- **Plugin Priority**: An integer value that determines execution order. Higher values run first. Same priority falls back to insertion order. The ordering mechanism that makes multi-plugin compositions deterministic.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Library consumers can bundle related policies, tools, and event observers into a single named plugin and register it with one call, reducing multi-step configuration to a single step.
- **SC-002**: Plugin execution order is deterministic and controllable via priority — consumers can guarantee that security plugins run before logging plugins.
- **SC-003**: Plugins compose cleanly with existing directly-registered policies and tools — no behavioral change for consumers who do not use plugins.
- **SC-004**: Consumers can inspect the registered plugin set (names, priorities, contributions) for debugging and operational visibility.
- **SC-005**: A plugin that panics during initialization or event observation does not crash the agent or prevent other plugins from functioning.
- **SC-006**: The plugin system adds zero overhead for agents that do not register any plugins.
- **SC-007**: Consumers can adopt plugins incrementally — adding a plugin to an existing agent configuration requires no changes to existing policy or tool registrations.

## Clarifications

### Session 2026-04-01

- Q: What happens when two different plugins both contribute a tool with the same name? → A: Plugin tools are namespace-prefixed as `{plugin_name}_{tool_name}`, preventing collisions entirely. Two plugins can both provide a tool named "export" — they become "alpha_export" and "beta_export". *(Separator was `.` in the original design; changed to `_` in #608 for provider compatibility.)*
- Q: Should plugins have a shutdown/cleanup callback? → A: No dedicated shutdown callback. Plugins use `on_event(AgentEnd)` for cleanup signaling or standard drop semantics for resource release. A dedicated `on_shutdown` can be added later if needed (backward-compatible addition).

## Assumptions

- Plugins are static registrations at construction time, not dynamic runtime additions. This is a deliberate constraint to avoid the complexity of hot-swapping policies and tools during a conversation. If dynamic plugin management is needed, it can be added in a future specification.
- The Plugin abstraction lives in the core `swink-agent` crate (behind a feature gate) since it references core types (policy traits, `AgentTool`, `AgentEvent`). Built-in plugin implementations (e.g., audit, telemetry) would live in their respective crates.
- Plugin priority is a signed integer (`i32`) to allow negative priorities for plugins that should run after the default (0). This matches common priority patterns in plugin systems (e.g., middleware stacks).
- The initialization callback receives a read-only reference to the agent. Plugins cannot mutate the agent during initialization — they observe and validate, not modify. Any modifications needed at init time should be expressed through the plugin's policy/tool contributions.
- In multi-agent scenarios (spec 009), each agent has its own independent plugin registry. Child agents do not inherit parent plugins. If a child agent needs the same plugins, the consumer must register them explicitly.
- Plugin-contributed tools are static — returned once during agent setup, not re-queried per turn. Dynamic tool addition (e.g., tools discovered at runtime from an MCP server) is a separate concern that would require a different mechanism.
- There are no built-in plugins shipped with the core crate in this specification. Built-in plugin examples (telemetry, audit) are documented as motivating use cases but their implementation is deferred to the crates that own those concerns (e.g., `swink-agent-policies` for audit).
