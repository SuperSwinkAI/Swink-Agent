# Feature Specification: Loop Policies & Observability

**Feature Branch**: `010-loop-policies-observability`
**Created**: 2026-03-20
**Status**: Draft
**Input**: Composable loop termination policies, stream middleware, structured event emission, metrics collection, post-turn hooks, budget guards, checkpoint snapshots, and OpenTelemetry integration. Cross-cutting infrastructure for governance, observability, and resumability. References: HLD Infrastructure Layer (LoopPolicy, StreamMiddleware, Emission, MetricsCollector, PostTurnHook, BudgetGuard, Checkpoint, OpenTelemetry).

## Supersession Notice

> **Partially superseded by [031-policy-slots](../031-policy-slots/spec.md).**
>
> The following concepts from this spec are replaced by the configurable policy slot system in 031:
> - **LoopPolicy** (MaxTurnsPolicy, CostCapPolicy, ComposedPolicy) → replaced by `PostTurnPolicy` slot (Slot 3) with opt-in `MaxTurnsPolicy` and `BudgetPolicy` implementations.
> - **PostTurnHook** (PostTurnAction: Continue/Stop/InjectMessages) → replaced by `PostTurnPolicy` slot (Slot 3). The same actions are expressed as `PolicyVerdict` variants (Continue, Stop, Inject).
> - **BudgetGuard** (pre-call cost/token enforcement) → replaced by `PreTurnPolicy` slot (Slot 1) with opt-in `BudgetPolicy` implementation.
>
> The following concepts from this spec **remain valid and are NOT affected by 031**:
> - **StreamMiddleware** (US2) — not a policy concern; stays as-is.
> - **MetricsCollector** (US3) — observation-only, explicitly excluded from the policy system (031 FR-022).
> - **Checkpoint** (US6) — persistence moves to an opt-in `CheckpointPolicy` in the PostTurn slot, but the `CheckpointStore` trait and snapshot format are unchanged.
> - **Structured event emission** (US3/FR-006) — not a policy concern; stays as-is.

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

A developer registers post-turn hooks that execute asynchronously after each turn completes. Hooks can persist state, send notifications, update dashboards, trigger side effects, stop the loop, or inject messages for the next turn. They run after the turn is finalized and return an action indicating how the loop should proceed.

**Why this priority**: Post-turn hooks enable integration with external systems without modifying loop internals.

**Independent Test**: Can be tested by registering a hook that records turn data and verifying it is called after each turn.

**Acceptance Scenarios**:

1. **Given** a post-turn hook, **When** a turn completes, **Then** the hook is called with the turn's data.
2. **Given** multiple hooks, **When** a turn completes, **Then** all hooks are called.
3. **Given** a hook returning `Continue`, **When** it runs, **Then** the loop proceeds normally.
4. **Given** a hook returning `Stop`, **When** it runs, **Then** the loop terminates after this turn.
5. **Given** a hook returning `InjectMessages`, **When** it runs, **Then** the returned messages are injected as pending for the next turn.

---

### User Story 5 - Guard Against Budget Overruns in Real Time (Priority: P2)

An operator sets budget limits (cost, tokens) that are checked before each LLM call. If any threshold is exceeded, the next LLM call is blocked and the agent loop terminates. This provides hard limits that take effect before execution, complementing loop policies that check at turn boundaries. Turn-based limits are handled by `MaxTurnsPolicy` (US1), not by BudgetGuard.

**Why this priority**: Budget guards provide pre-call safety — loop policies check after a turn completes (by then tokens/cost are already spent), but budget guards prevent the next call from starting.

**Independent Test**: Can be tested by setting a token budget below the expected total usage and verifying the agent is blocked when the budget is exceeded before the next LLM call.

**Acceptance Scenarios**:

1. **Given** a cost budget, **When** accumulated cost exceeds it before an LLM call, **Then** the call is blocked with `BudgetExceeded::Cost`.
2. **Given** a token budget, **When** accumulated tokens exceed it before an LLM call, **Then** the call is blocked with `BudgetExceeded::Tokens`.

---

### User Story 6 - Save and Restore Loop State (Priority: P3)

A developer enables checkpoints so the agent's loop state is snapshotted at turn boundaries. If the agent is interrupted, it can be resumed from the last checkpoint rather than replaying from the beginning.

**Why this priority**: Checkpoints enable resumability for long-running agents, but most agents complete without interruption.

**Independent Test**: Can be tested by running an agent for 3 turns, saving a checkpoint, and verifying it can be restored.

**Acceptance Scenarios**:

1. **Given** checkpointing enabled, **When** a turn completes, **Then** the loop state is captured as a checkpoint.
2. **Given** a checkpoint, **When** the agent is restored from it, **Then** it resumes from the checkpointed state.

---

### User Story 7 - OpenTelemetry Integration (Priority: P2) — C9

An operator enables OpenTelemetry-compliant tracing so that Swink agents emit structured spans and attributes compatible with standard observability backends (Datadog, Grafana, Honeycomb, Jaeger). The integration is opt-in via a feature gate and works alongside the existing `MetricsCollector` and `AgentEvent` systems without replacing them.

**Why this priority**: Production teams need agents to fit into existing observability stacks. Without OTel, operators must build custom adapters from `MetricsCollector` or `AgentEvent` data. OTel gives them zero-custom-code integration with any OTLP-compatible backend.

**Independent Test**: Can be tested by enabling the `otel` feature, running an agent with a mock OTel exporter, and verifying the exporter receives the expected span hierarchy and attributes.

**Acceptance Scenarios**:

1. **Given** the `otel` feature is enabled and a `TracerProvider` is configured, **When** the agent runs a prompt, **Then** a root span `agent.run` is created covering the full prompt-to-response lifecycle.
2. **Given** an active `agent.run` span, **When** a turn begins and ends, **Then** a child span `agent.turn` is created with attributes `agent.turn_index` and `agent.stop_reason`.
3. **Given** an active `agent.turn` span, **When** the LLM streaming call executes, **Then** a child span `agent.llm_call` is created with attributes `agent.model`, `agent.tokens.input`, `agent.tokens.output`, and `agent.cost.total`.
4. **Given** an active `agent.turn` span, **When** a tool executes, **Then** a child span `agent.tool.{name}` is created with attributes `agent.tool.name`, the tool duration, and success/error status.
5. **Given** the `otel` feature is **not** enabled, **When** the crate is compiled, **Then** no OpenTelemetry dependencies are included and there is zero runtime overhead.
6. **Given** OTel tracing is enabled, **When** a `MetricsCollector` is also configured, **Then** both operate independently — OTel does not suppress or duplicate the `MetricsCollector` callback.

---

### Edge Cases

- What happens when a policy and a budget guard both trigger — **[Superseded by 031]** Both are now policies in slots. BudgetPolicy runs in PreTurn (Slot 1), MaxTurnsPolicy runs in PostTurn (Slot 3). They are independent policies at different slots and cannot trigger simultaneously.
- How does the system handle a post-turn hook that panics — the panic is caught and logged; the hook's action is skipped and the loop continues. A panicking hook does not crash the agent.
- What happens when the checkpoint store fails to persist — `CheckpointStore` returns `io::Result`; failures propagate as errors. The caller decides whether to continue or stop.
- How does stream middleware interact with retry — each retry re-invokes StreamFn, producing a new stream that gets wrapped by middleware again. Retries are also wrapped.
- What happens when the OTel exporter is unavailable — spans are still created via the `tracing` crate; the exporter failure is handled by the OTel SDK (typically logged and dropped). The agent loop is never blocked or slowed by exporter issues.
- How does OTel interact with model fallback — when a model fallback occurs, the failed `agent.llm_call` span ends with an error status, and a new `agent.llm_call` span is created for the fallback model. Both appear as children of the same `agent.turn`.
- What happens with concurrent tool executions — each tool gets its own `agent.tool.{name}` span. Concurrent tools produce overlapping child spans under the same `agent.turn` parent, which is standard OTel behavior.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST provide a loop policy trait with a method to determine whether the loop should continue after a turn.
- **FR-002**: System MUST provide built-in policies: maximum turns, cost cap, and composed (multiple policies, any-trigger termination).
- **FR-003**: Closures MUST be usable as loop policies for ad-hoc rules.
- **FR-004**: System MUST provide stream middleware that wraps the streaming output using the decorator pattern.
- **FR-005**: Stream middleware MUST be composable — multiple middleware can be chained.
- **FR-006**: System MUST provide structured event emission for enriched event payloads.
- **FR-007**: System MUST provide a metrics collector that records turn-level and tool-execution-level metrics (latency, tokens, cost, count).
- **FR-008**: System MUST provide async post-turn hooks that execute after each turn and return a `PostTurnAction` (Continue, Stop, or InjectMessages) to influence loop behavior.
- **FR-009**: System MUST provide a budget guard that monitors cost, token, and turn thresholds in real time during stream collection and cancels the agent when any threshold is exceeded.
- **FR-010**: System MUST provide checkpoint snapshots at turn boundaries with save and restore capability.
- **FR-011**: System MUST provide opt-in OpenTelemetry integration behind a `feature = "otel"` gate that emits structured spans with a hierarchy of `agent.run` → `agent.turn` → `agent.llm_call` / `agent.tool.{name}`.
- **FR-012**: OTel spans MUST carry semantic attributes: `agent.model`, `agent.turn_index`, `agent.tokens.input`, `agent.tokens.output`, `agent.cost.total`, `agent.tool.name`, `agent.stop_reason`.
- **FR-013**: OTel integration MUST use the `tracing` crate with `tracing-opentelemetry` as the bridge layer, leveraging the existing `tracing` dependency rather than importing `opentelemetry` APIs directly into the core crate.
- **FR-014**: OTel integration MUST coexist with the existing `MetricsCollector` trait — enabling OTel MUST NOT disable, replace, or duplicate `MetricsCollector` callbacks.
- **FR-015**: When the `otel` feature is disabled, there MUST be zero additional dependencies and zero runtime overhead from OTel instrumentation.
- **FR-016**: OTel span attributes MUST be limited to structural metadata (tool name, model ID, token counts, cost, turn index, stop reason). Spans MUST NOT include prompt text, tool arguments, or tool results to prevent data leakage to external backends.
- **FR-017**: The `otel` feature MUST only emit OTel traces (spans). OTel Metrics (counters, histograms) are explicitly out of scope — the `MetricsCollector` trait serves that role.

### Key Entities

- **LoopPolicy**: Trait for loop termination decisions — MaxTurnsPolicy, CostCapPolicy, ComposedPolicy.
- **StreamMiddleware**: Decorator wrapping the streaming output for event interception/transformation.
- **MetricsCollector**: Records turn and tool execution metrics.
- **PostTurnHook**: Callback executed after each turn for side effects.
- **BudgetGuard**: Real-time cost/token/turn monitor that cancels the agent on threshold breach.
- **Checkpoint**: Serializable snapshot of loop state at a turn boundary.
- **OtelInitConfig**: Configuration struct for the convenience `init_otel_layer()` function (service name, OTLP endpoint). Feature-gated behind `otel`.
- **init_otel_layer()**: Convenience function returning a `tracing_subscriber::Layer` that bridges `tracing` spans to OpenTelemetry via `tracing-opentelemetry` with an OTLP gRPC exporter.

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
- **SC-009**: With `otel` feature enabled, a mock OTel exporter captures the correct span hierarchy (`agent.run` → `agent.turn` → `agent.llm_call` / `agent.tool.{name}`).
- **SC-010**: OTel span attributes (`agent.model`, `agent.turn_index`, `agent.tokens.input`, `agent.tokens.output`, `agent.cost.total`, `agent.tool.name`, `agent.stop_reason`) are correctly populated from loop state and `TurnMetrics` data.
- **SC-011**: With `otel` feature disabled, the crate compiles without `tracing-opentelemetry` or `opentelemetry` in the dependency tree.

## Clarifications

### Session 2026-03-20

- Q: Should PostTurnHook be async with control flow actions (matching impl) or sync/observe-only (matching old spec)? → A: **[Superseded by 031]** PostTurnHook is replaced by sync PostTurnPolicy. Control flow actions are expressed as PolicyVerdict (Continue, Stop, Inject).
- Q: Should panicking post-turn hooks crash the loop? → A: No — catch panic, log it, skip the hook's action, continue the loop.
- Q: Policy vs budget guard precedence? → A: **[Superseded by 031]** Both are now policies in separate slots (PreTurn vs PostTurn). Independent by design.
- Q: Checkpoint store failure behavior? → A: io::Result propagates; caller decides.
- Q: Stream middleware + retry? → A: Each retry produces new stream, re-wrapped by middleware.

### Session 2026-03-31

- Q: Should OTel span attributes include content (prompts, tool arguments, tool results) or only structural metadata? → A: Structural metadata only — spans carry tool name, model ID, token counts, cost, turn index, and stop reason. Never include prompt text, tool arguments, or tool results as span attributes to prevent accidental data leakage to external OTel backends.
- Q: Which `opentelemetry` crate version should the `otel` feature target? → A: `>=0.29` — target `opentelemetry 0.29+` / `opentelemetry_sdk 0.29+` / `opentelemetry-otlp 0.29+` aligned with `tracing-opentelemetry 0.30+`. Use latest compatible versions in workspace deps.
- Q: Should the `otel` feature emit OTel Metrics (counters/histograms) in addition to traces? → A: Traces only — OTel Metrics are out of scope. The existing `MetricsCollector` trait already provides structured per-turn metrics that users can push to any system. OTel backends can derive metrics from span data.
- Q: Should `init_otel_layer` support both gRPC and HTTP OTLP protocols? → A: gRPC only in the convenience helper. Users who need HTTP OTLP can compose their own `tracing_subscriber` pipeline — they are sophisticated enough to do so.
- Q: Should OTel integration use the `opentelemetry` crate directly or go through `tracing`? → A: Use `tracing` crate instrumentation in the core loop (spans, fields) with `tracing-opentelemetry` as the bridge layer. This avoids importing the `opentelemetry` API into the core crate and leverages the existing `tracing` dependency. The `tracing-opentelemetry` dep is feature-gated behind `otel`.
- Q: Should OTel replace MetricsCollector? → A: No — they coexist. MetricsCollector is a push-based Rust trait for in-process consumers. OTel is for external observability backends. Users can use both, either, or neither.
- Q: Should there be a helper for configuring the OTel pipeline? → A: Yes — provide a convenience function `init_otel_layer()` that returns a configured `tracing_subscriber::Layer` with `tracing-opentelemetry` and OTLP exporter. This is opt-in setup code, not mandatory.
- Q: Push (OTLP exporter) vs pull (Prometheus)? → A: The primary integration is push via OTLP (gRPC/HTTP). Prometheus pull can be supported by users configuring a Prometheus exporter as their OTel exporter — no special support needed from the core crate.

## Assumptions

- **[Superseded by 031]** Loop policies and budget guards are replaced by configurable policy slots. PreTurn policies (Slot 1) run before each LLM call. PostTurn policies (Slot 3) run after each turn. All policies are sync and return PolicyVerdict.
- **[Superseded by 031]** Post-turn hooks are replaced by PostTurnPolicy (sync, returns PolicyVerdict with Continue/Stop/Inject variants).
- Budget enforcement uses the same cancellation token mechanism as manual abort (unchanged).
- Checkpoints are opt-in — when not configured, no checkpoint overhead is incurred.
- Metrics are collected in-memory by default; persistence is the caller's responsibility.
- OpenTelemetry integration builds on the existing `tracing` crate already used throughout the codebase for diagnostics.
- OTel span semantics follow emerging LLM observability conventions (no formal OpenTelemetry semantic convention for LLM agents exists yet, so we define `agent.*` attributes).
- The `tracing-opentelemetry` (>=0.30) and `opentelemetry` (>=0.29) crates are only compiled when `feature = "otel"` is enabled.
