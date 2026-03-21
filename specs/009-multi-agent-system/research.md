# Research: Multi-Agent System

**Feature**: 009-multi-agent-system | **Date**: 2026-03-20

## Design Decisions

### D1: RwLock-based registry with replace-on-duplicate

**Decision**: `AgentRegistry` uses `Arc<RwLock<HashMap<String, AgentRef>>>` where `AgentRef = Arc<tokio::sync::Mutex<Agent>>`. Registering a duplicate name silently replaces the previous entry (standard `HashMap::insert` behavior).

**Rationale**: RwLock allows concurrent reads (multiple `get()` calls) with exclusive writes (register/remove). All operations are fast HashMap lookups with no `.await` held across the lock, so `std::sync::RwLock` is correct over `tokio::sync::RwLock`. The inner `AgentRef` uses `tokio::sync::Mutex` because agent operations (prompt, steer) are async. Replace-on-duplicate enables re-registration after restart or reconfiguration without requiring explicit removal first.

**Alternatives rejected**:
- **Reject duplicate names**: Would require callers to remove-then-register, adding ceremony for the common restart case. HashMap::insert semantics are well-understood.
- **`tokio::sync::RwLock` for the outer map**: Unnecessary async overhead — the lock is held only for the duration of a HashMap get/insert, never across an await point.
- **`DashMap`**: External dependency for concurrent HashMap. RwLock + HashMap is simpler, sufficient for expected scale, and avoids adding a dependency.

### D2: Mutex-guarded Vec mailbox (non-blocking)

**Decision**: `AgentMailbox` wraps `Arc<Mutex<Vec<AgentMessage>>>`. `send()` pushes to the Vec (non-blocking), `drain()` takes all pending messages via `std::mem::take`. No deadlock risk with circular messaging.

**Rationale**: The mailbox is a simple inbox pattern — senders push, the owner drains. A `Mutex<Vec>` is the simplest correct implementation. Since `send()` only does a push (O(1) amortized), the lock is held for nanoseconds, making it effectively non-blocking. Circular messaging (A sends to B, B sends to A) works because neither agent blocks waiting for the other to process — the messages just accumulate in their respective mailboxes.

**Alternatives rejected**:
- **`tokio::sync::mpsc` channel**: Requires pre-creating the channel and sharing the sender. The Vec approach is simpler for fire-and-forget messaging where the recipient polls at its own pace.
- **Lock-free queue (crossbeam)**: Over-engineered for the expected contention (typically one sender at a time, one consumer). Adds a dependency for minimal benefit.

### D3: SubAgent as AgentTool impl with cancellation propagation

**Decision**: `SubAgent` implements `AgentTool`. Each `execute()` call constructs a fresh `Agent` via an `options_factory` closure, runs `prompt_text()` with the provided prompt, and uses `tokio::select!` to propagate the parent's `CancellationToken`. A `map_result` closure converts `AgentResult` to `AgentToolResult`.

**Rationale**: Fresh agent per execution avoids state leakage between invocations. The factory pattern (closure returning `AgentOptions`) allows full customization of the child agent. `tokio::select!` is the idiomatic way to race the agent execution against cancellation. The default result mapper extracts text from the last assistant message, which covers the common case; custom mappers handle structured output or multi-message results.

**Alternatives rejected**:
- **Persistent child agent (reuse across calls)**: State from previous calls would leak into subsequent ones. The factory pattern is clean and predictable.
- **Thread-based cancellation (interrupt)**: Rust/tokio do not support thread interruption. `CancellationToken` + `select!` is the correct async cancellation pattern.
- **Channel-based result passing**: Unnecessary indirection — the tool execute future already returns the result.

### D4: Orchestrator with supervisor policy (Restart/Escalate/Stop)

**Decision**: `AgentOrchestrator` manages named agents via a `HashMap<String, AgentEntry>`. Each entry holds an `OptionsFactoryArc`, parent/child relationships, and max restart count. `spawn()` runs the agent in a `tokio::spawn` task that listens for `AgentRequest` messages via `mpsc`. `SupervisorPolicy::on_agent_error()` returns `SupervisorAction` (Restart/Stop/Escalate). `DefaultSupervisor` restarts on retryable errors and stops otherwise.

**Rationale**: The orchestrator is a convenience layer for complex multi-agent systems. The factory pattern allows restarting agents with fresh state. mpsc channels provide request/response messaging with backpressure. The supervisor policy trait allows customization — the default covers the common case (retry transient errors, stop on permanent ones). Parent/child hierarchy is tracked for future use (cascading shutdown, error escalation).

**Alternatives rejected**:
- **Actor framework (e.g., actix)**: Heavy dependency for what amounts to a supervised task pool. The orchestrator is ~200 lines of focused code.
- **Flat agent pool (no hierarchy)**: Parent/child tracking is cheap (two fields) and enables cascading operations later.
- **Restart inside the agent loop itself**: Would conflate retry (same agent, same context) with restart (fresh agent, clean state). Separating them in the supervisor keeps concerns clear.

### D5: Inter-agent messaging via `send_to` free function

**Decision**: `send_to(registry, agent_name, message)` is an async free function that looks up the agent by name in the registry, acquires the tokio::sync::Mutex lock, and calls `agent.steer(message)` to inject into the steering queue. Returns `AgentError::Plugin` if the agent is not found.

**Rationale**: Using the agent's existing steering queue means messages are processed naturally within the agent loop — no additional polling or channel infrastructure needed. The free function keeps `AgentMailbox` and `send_to` as separate, composable primitives. `AgentMailbox` is a standalone inbox; `send_to` bridges registry lookup with agent steering. Callers can use either or both depending on their architecture.

**Alternatives rejected**:
- **Method on AgentRegistry**: Would couple the registry to the messaging implementation. The free function is more flexible.
- **Direct mailbox delivery (bypassing steering)**: Would require agents to poll their mailbox separately. Reusing the steering queue integrates with the existing agent loop.
