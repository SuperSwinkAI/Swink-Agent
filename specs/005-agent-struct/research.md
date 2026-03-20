# Research: Agent Struct & Public API

**Feature**: 005-agent-struct | **Date**: 2026-03-20

## Design Decisions

### D1: Single-invocation concurrency guard

**Decision**: The Agent allows only one active invocation at a time. A second call while running returns `Err(AgentError::AlreadyRunning)`.

**Rationale**: The Agent owns mutable state (messages, stream_message, pending_tool_calls). Allowing concurrent invocations would require either interior mutability everywhere or complex synchronization. Since the Agent takes `&mut self` for all invocation methods, Rust's borrow checker enforces this at compile time for most cases. The runtime `is_running` flag catches edge cases (e.g., calling prompt from within an event subscriber callback that holds a reference).

**Alternatives rejected**:
- **Queue multiple invocations**: Adds complexity with unclear semantics (do queued prompts share history?). Users who need this can use `AgentHandle::spawn` or manage their own queue.
- **Clone-per-invocation**: Would require `Agent: Clone` and lose state sharing. Defeats the purpose of a stateful wrapper.

### D2: Three invocation modes via stream-first architecture

**Decision**: All invocations funnel through `start_loop()` which returns a `Pin<Box<dyn Stream<Item = AgentEvent> + Send>>`. `prompt_async` collects the stream. `prompt_sync` creates a fresh tokio Runtime and blocks on collection.

**Rationale**: Stream-first means the streaming path is the canonical one. Async and sync are convenience wrappers, not separate code paths. This eliminates divergence bugs.

**Alternatives rejected**:
- **Separate implementations per mode**: Would triple the maintenance surface and invite divergence.
- **Channel-based instead of Stream**: Streams compose better with futures and allow backpressure. The channel approach was used for `AgentHandle` where ownership transfer is needed.

### D3: Queues use `Arc<Mutex<Vec<>>>` with poison recovery

**Decision**: Steering and follow-up queues are `Arc<Mutex<Vec<AgentMessage>>>`. Lock acquisition uses `unwrap_or_else(PoisonError::into_inner)` to never panic on poisoned locks.

**Rationale**: Queues must be shared between the Agent (which enqueues) and the loop (which drains via `QueueMessageProvider`). `Arc<Mutex<>>` is the simplest correct approach. Poison recovery ensures that a panic in one thread does not permanently lock the queue.

**Alternatives rejected**:
- **`tokio::sync::Mutex`**: Unnecessary async overhead for a fast Vec operation.
- **Lock-free queue (crossbeam)**: Over-engineered for the expected contention level (one producer, one consumer, small messages).

### D4: Subscriber panic isolation via `catch_unwind`

**Decision**: `dispatch_event` in `ListenerRegistry` wraps each callback invocation in `std::panic::catch_unwind`. Panicking subscribers are automatically removed.

**Rationale**: A UI subscriber bug should not crash the agent loop. Auto-removal prevents repeated panics on every event. This is documented in CLAUDE.md as a QA-discovered behavior.

### D5: Structured output via synthetic tool injection

**Decision**: `structured_output()` temporarily injects a `__structured_output` tool, runs a normal prompt loop, extracts and validates the tool call arguments, then removes the tool. Invalid responses trigger retry via `continue_async()`.

**Rationale**: Reusing the normal tool-call flow means structured output works with any provider that supports tool calls. No special provider API needed. Retry via continue gives the LLM its previous invalid attempt as context.

**Alternatives rejected**:
- **Provider-native structured output (e.g., OpenAI JSON mode)**: Provider-specific, violates Provider Agnosticism principle.
- **Post-hoc JSON extraction from text**: Fragile, no schema validation feedback loop.

### D6: Continue validation guards

**Decision**: `validate_continue()` returns `NoMessages` if history is empty, and `InvalidContinue` if the last message is an assistant message with no pending queue messages.

**Rationale**: Continuing from an assistant message with nothing queued would just re-send the same context, likely producing the same response. If there are pending steering/follow-up messages, the continue is allowed because those messages change the context.

### D7: Agent identity via `AgentId`

**Decision**: Each Agent gets a unique `AgentId` from `AgentId::next()` (atomic counter). This is used for registry lookup and multi-agent coordination.

**Rationale**: Agents need stable identity for the registry system (`src/registry.rs`) and for sub-agent orchestration (`src/sub_agent.rs`).
