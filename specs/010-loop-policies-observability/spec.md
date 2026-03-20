# Feature Specification: Loop Policies & Observability

**Feature Branch**: `010-loop-policies-observability`
**Created**: 2026-03-20
**Status**: Draft
**Input**: Composable loop termination policies, stream middleware, structured event emission, metrics collection, post-turn hooks, budget guards, and checkpoint snapshots. Cross-cutting infrastructure for governance, observability, and resumability. References: HLD Infrastructure Layer (LoopPolicy, StreamMiddleware, Emission, MetricsCollector, PostTurnHook, BudgetGuard, Checkpoint).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Limit Agent Turns and Cost (Priority: P1)

An operator configures policies to prevent runaway agent loops. They set a maximum turn count and a cost cap. The policies compose — the agent stops when either limit is hit. Simple policies can also be expressed as closures for ad-hoc rules.

**Why this priority**: Without governance, an agent could loop indefinitely or spend unbounded money. Policies are the safety net.

**Independent Test**: Can be tested by configuring a max-turns policy of 3 and running an agent that would take 5 turns, verifying it stops at 3.

**Acceptance Scenarios**:

1. **Given** a max-turns policy of N, **When** the agent reaches N turns, **Then** the loop terminates.
2. **Given** a cost-cap policy, **When** accumulated cost exceeds the cap, **Then** the loop terminates.
3. **Given** two composed policies, **When** either policy triggers, **Then** the loop terminates.
4. **Given** a closure-based policy, **When** it returns "stop," **Then** the loop terminates.

---

### User Story 2 - Intercept the Output Stream (Priority: P2)

A developer wraps the streaming output with middleware that intercepts, transforms, or enriches assistant message events before they reach the caller. Stream middleware follows the decorator pattern — composable without modifying the inner streaming function.

**Why this priority**: Stream middleware enables logging, content filtering, and event enrichment without coupling those concerns to provider adapters.

**Independent Test**: Can be tested by wrapping a mock stream with middleware that adds a prefix to text deltas and verifying the caller sees the modified events.

**Acceptance Scenarios**:

1. **Given** stream middleware, **When** events flow through it, **Then** the middleware can inspect, modify, or filter events.
2. **Given** multiple composed middleware, **When** events flow through, **Then** each middleware processes events in order.

---

### User Story 3 - Collect Metrics on Agent Performance (Priority: P2)

An operator collects metrics on agent performance: turn count, turn latency, tool execution count and latency, token consumption, and cost per turn. These metrics are available after each turn for dashboards, alerting, or logging.

**Why this priority**: Observability is essential for production monitoring, but the agent works without it.

**Independent Test**: Can be tested by running a multi-turn conversation and verifying the metrics collector reports correct counts and latencies.

**Acceptance Scenarios**:

1. **Given** a metrics collector, **When** a turn completes, **Then** turn-level metrics (latency, tokens, cost) are recorded.
2. **Given** tool executions, **When** they complete, **Then** per-tool metrics (name, latency, success/failure) are recorded.
3. **Given** collected metrics, **When** they are queried, **Then** they report accurate totals and per-turn breakdowns.

---

### User Story 4 - Execute Logic After Each Turn (Priority: P2)

A developer registers post-turn hooks that execute after each turn completes. Hooks can persist state, send notifications, update dashboards, or trigger side effects. They run after the turn is finalized and do not affect the loop's control flow.

**Why this priority**: Post-turn hooks enable integration with external systems without modifying loop internals.

**Independent Test**: Can be tested by registering a hook that records turn data and verifying it is called after each turn.

**Acceptance Scenarios**:

1. **Given** a post-turn hook, **When** a turn completes, **Then** the hook is called with the turn's data.
2. **Given** multiple hooks, **When** a turn completes, **Then** all hooks are called.
3. **Given** a hook, **When** it runs, **Then** it does not affect the loop's control flow or next turn.

---

### User Story 5 - Guard Against Budget Overruns in Real Time (Priority: P2)

An operator sets real-time budget limits (cost, tokens, turns) that are monitored during stream collection. If any threshold is exceeded mid-run, the agent is cancelled via its cancellation token. This provides hard limits that take effect during execution, not just at turn boundaries.

**Why this priority**: Budget guards provide real-time safety — loop policies check at turn boundaries but budget guards can cancel mid-stream.

**Independent Test**: Can be tested by setting a token budget below the expected response size and verifying the agent is cancelled when the budget is exceeded.

**Acceptance Scenarios**:

1. **Given** a cost budget, **When** accumulated cost exceeds it during streaming, **Then** the agent is cancelled.
2. **Given** a token budget, **When** accumulated tokens exceed it, **Then** the agent is cancelled.
3. **Given** a turn budget, **When** the turn count exceeds it, **Then** the agent is cancelled.

---

### User Story 6 - Save and Restore Loop State (Priority: P3)

A developer enables checkpoints so the agent's loop state is snapshotted at turn boundaries. If the agent is interrupted, it can be resumed from the last checkpoint rather than replaying from the beginning.

**Why this priority**: Checkpoints enable resumability for long-running agents, but most agents complete without interruption.

**Independent Test**: Can be tested by running an agent for 3 turns, saving a checkpoint, and verifying it can be restored.

**Acceptance Scenarios**:

1. **Given** checkpointing enabled, **When** a turn completes, **Then** the loop state is captured as a checkpoint.
2. **Given** a checkpoint, **When** the agent is restored from it, **Then** it resumes from the checkpointed state.

---

### Edge Cases

- What happens when a policy and a budget guard both trigger at the same time — which takes precedence?
- How does the system handle a post-turn hook that panics — is the loop affected?
- What happens when checkpointing is enabled but the checkpoint store fails to persist — does the agent continue or stop?
- How does stream middleware interact with the retry mechanism — are retry attempts also wrapped?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST provide a loop policy trait with a method to determine whether the loop should continue after a turn.
- **FR-002**: System MUST provide built-in policies: maximum turns, cost cap, and composed (multiple policies, any-trigger termination).
- **FR-003**: Closures MUST be usable as loop policies for ad-hoc rules.
- **FR-004**: System MUST provide stream middleware that wraps the streaming output using the decorator pattern.
- **FR-005**: Stream middleware MUST be composable — multiple middleware can be chained.
- **FR-006**: System MUST provide structured event emission for enriched event payloads.
- **FR-007**: System MUST provide a metrics collector that records turn-level and tool-execution-level metrics (latency, tokens, cost, count).
- **FR-008**: System MUST provide post-turn hooks that execute after each turn without affecting loop control flow.
- **FR-009**: System MUST provide a budget guard that monitors cost, token, and turn thresholds in real time during stream collection and cancels the agent when any threshold is exceeded.
- **FR-010**: System MUST provide checkpoint snapshots at turn boundaries with save and restore capability.

### Key Entities

- **LoopPolicy**: Trait for loop termination decisions — MaxTurnsPolicy, CostCapPolicy, ComposedPolicy.
- **StreamMiddleware**: Decorator wrapping the streaming output for event interception/transformation.
- **MetricsCollector**: Records turn and tool execution metrics.
- **PostTurnHook**: Callback executed after each turn for side effects.
- **BudgetGuard**: Real-time cost/token/turn monitor that cancels the agent on threshold breach.
- **Checkpoint**: Serializable snapshot of loop state at a turn boundary.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: MaxTurnsPolicy correctly terminates the loop at the configured turn count.
- **SC-002**: CostCapPolicy correctly terminates the loop when cost exceeds the cap.
- **SC-003**: Composed policies terminate when any constituent policy triggers.
- **SC-004**: Stream middleware correctly intercepts and transforms events.
- **SC-005**: Metrics collector accurately reports turn count, latency, token usage, and cost.
- **SC-006**: Post-turn hooks execute after every turn without affecting loop control flow.
- **SC-007**: Budget guard cancels the agent in real time when any threshold is exceeded.
- **SC-008**: Checkpoints can be saved and restored, enabling agent resumption from a prior state.

## Assumptions

- Loop policies are checked at turn boundaries, not mid-stream. Budget guards handle mid-stream enforcement.
- Post-turn hooks run synchronously after turn finalization but before the next turn begins.
- Budget guard cancellation uses the same cancellation token mechanism as manual abort.
- Checkpoints are opt-in — when not configured, no checkpoint overhead is incurred.
- Metrics are collected in-memory by default; persistence is the caller's responsibility.
