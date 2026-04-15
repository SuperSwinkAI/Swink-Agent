# Feature Specification: Multi-Agent System

**Feature Branch**: `009-multi-agent-system`
**Created**: 2026-03-20
**Status**: Draft
**Input**: Agent registry for named lookup, inter-agent messaging, orchestrator for multi-agent supervision, and SubAgent tool wrapper for agent-as-tool composition. References: HLD Catalogs & Registries (AgentRegistry, AgentMailbox), HLD Infrastructure (AgentOrchestrator), HLD Implementations (SubAgent).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Register and Look Up Named Agents (Priority: P1)

A developer builds a multi-agent system where each agent has a unique name and role. They register agents in a shared registry and look them up by name at runtime. The registry is thread-safe so agents running concurrently can discover each other.

**Why this priority**: The registry is the foundation for all multi-agent coordination. Without named lookup, agents cannot find or communicate with each other.

**Independent Test**: Can be tested by registering multiple agents with unique names and verifying they can be looked up by name from different threads.

**Acceptance Scenarios**:

1. **Given** an empty registry, **When** an agent is registered with a name, **Then** it can be looked up by that name.
2. **Given** a registry with agents, **When** a lookup is performed for a non-existent name, **Then** it returns nothing.
3. **Given** a registry, **When** agents are registered and looked up from different threads, **Then** all operations are thread-safe.
4. **Given** a registered agent, **When** it is removed from the registry, **Then** subsequent lookups return nothing.

---

### User Story 2 - Send Messages Between Agents (Priority: P1)

A developer enables agents to communicate by sending messages to each other via a mailbox system. An agent sends a message to another agent by name, and the recipient processes it asynchronously. This decouples agents from direct references — they only need to know each other's names.

**Why this priority**: Inter-agent messaging is the primary communication mechanism in multi-agent systems. Without it, agents can only interact through the tool system.

**Independent Test**: Can be tested by creating two agents, sending a message from one to the other, and verifying the recipient processes it.

**Acceptance Scenarios**:

1. **Given** two registered agents, **When** agent A sends a message to agent B by name, **Then** agent B receives the message.
2. **Given** a message sent to a non-existent agent, **When** delivery is attempted, **Then** a delivery failure is reported.
3. **Given** asynchronous messaging, **When** a message is sent, **Then** the sender does not block waiting for the recipient to process it.

---

### User Story 3 - Invoke an Agent as a Tool (Priority: P2)

A developer wraps a child agent as a tool that a parent agent can invoke. When the parent agent calls the tool, the child agent runs with the provided arguments and returns its result. This bridges the tool system with multi-agent composition — the parent doesn't know it's talking to another agent.

**Why this priority**: SubAgent is the most natural way to compose agents — a parent delegates subtasks to specialized child agents via the familiar tool interface.

**Independent Test**: Can be tested by creating a parent agent with a SubAgent tool, invoking it, and verifying the child agent runs and returns a result.

**Acceptance Scenarios**:

1. **Given** a child agent wrapped as a SubAgent tool, **When** the parent invokes it, **Then** the child agent runs with the provided prompt.
2. **Given** a SubAgent tool, **When** it is listed in the parent's tools, **Then** it has a name, description, and parameter schema like any other tool.
3. **Given** a SubAgent execution, **When** the parent's cancellation token is triggered, **Then** the child agent's execution is also cancelled.

---

### User Story 4 - Supervise Multiple Agents (Priority: P3)

A developer uses an orchestrator to manage the lifecycle of multiple agents. The orchestrator handles agent creation, monitors their status, coordinates delegation of tasks, and manages shutdown. This provides a higher-level abstraction over the registry and messaging primitives.

**Why this priority**: The orchestrator is a convenience layer over registry + messaging — useful for complex multi-agent systems but not required for basic composition.

**Independent Test**: Can be tested by creating an orchestrator, adding agents, delegating tasks, and verifying lifecycle management.

**Acceptance Scenarios**:

1. **Given** an orchestrator, **When** agents are added, **Then** they are registered and their lifecycle is managed.
2. **Given** an orchestrator with agents, **When** a task is delegated, **Then** it is routed to the appropriate agent.
3. **Given** an orchestrator, **When** shutdown is requested, **Then** all managed agents are stopped cleanly.

---

### Edge Cases

- What happens when two agents are registered with the same name — the registration panics and is rejected; duplicate names are not allowed.
- How does the system handle circular messaging — no deadlock; mailbox send is non-blocking (push to `Arc<Mutex<Vec>>`). Circular messaging works without issues.
- What happens when a SubAgent tool call times out — the parent's cancellation token cancels the child via `tokio::select!`; child agent is aborted and returns an error result.
- How does the orchestrator handle an agent that panics/errors — the supervisor policy's `on_agent_error` decides: Restart (recreate agent, up to max_restarts) or Escalate (report error, keep agent alive).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST provide a thread-safe agent registry for registering, looking up, and removing agents by unique name.
- **FR-002**: Registry lookups MUST return a reference to the agent that can be used to interact with it.
- **FR-003**: System MUST provide an asynchronous inter-agent messaging system where agents send messages to other agents by name.
- **FR-004**: Messaging MUST be decoupled from the registry — agents need only know recipient names, not hold direct references.
- **FR-005**: Message delivery to a non-existent agent MUST report a failure rather than silently dropping the message.
- **FR-006**: System MUST provide a SubAgent wrapper that presents a child agent as a tool implementable by a parent agent.
- **FR-007**: SubAgent MUST implement the standard tool trait (name, description, schema, execute).
- **FR-008**: SubAgent MUST propagate cancellation from the parent to the child agent.
- **FR-009**: System MUST provide an orchestrator for multi-agent lifecycle management, task delegation, and coordinated shutdown.

### Key Entities

- **AgentRegistry**: Thread-safe registry for named agent lookup — register, find, remove.
- **AgentId**: Unique identifier for a registered agent.
- **AgentMailbox**: Asynchronous messaging channel for inter-agent communication.
- **SubAgent**: Tool wrapper that invokes a child agent — bridges tool system with multi-agent composition.
- **AgentOrchestrator**: Supervisor managing lifecycle, delegation, and shutdown of multiple agents.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Agents can be registered, looked up by name, and removed from the registry — all thread-safe.
- **SC-002**: Messages sent between agents are delivered asynchronously without blocking the sender.
- **SC-003**: SubAgent tools execute correctly when invoked by a parent agent and propagate cancellation.
- **SC-004**: The orchestrator manages agent lifecycle including creation, task delegation, and clean shutdown.
- **SC-005**: Messaging to a non-existent agent produces a clear failure indication.

## Clarifications

### Session 2026-03-20

- Q: Should duplicate agent name registration replace or reject? → A: Reject — duplicate registration panics; agent names must be unique within a registry.
- Q: Does circular messaging cause deadlock? → A: No; mailbox send is non-blocking (push to mutex-guarded Vec).
- Q: Does SubAgent timeout cancel the child? → A: Yes; parent's cancellation token cancels child via `tokio::select!`.
- Q: How does orchestrator handle agent errors? → A: Supervisor policy decides: Restart (up to max_restarts) or Escalate.

## Assumptions

- Agent names are unique within a registry. Registering a duplicate name panics and is rejected.
- Inter-agent messaging is asynchronous and non-blocking — fire-and-forget with delivery confirmation.
- SubAgent wraps a complete agent invocation (prompt → run → result), not individual tool calls.
- The orchestrator is optional — simple multi-agent setups can use registry + messaging directly.
