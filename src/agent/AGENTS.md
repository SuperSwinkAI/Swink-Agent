# Agent Lessons

## Core Mechanics
- `dispatch_event` catches panics via `catch_unwind`: panicking subscribers auto-removed, panicking forwarders logged and skipped.
- `in_flight_llm_messages` filters out `CustomMessage`. Queues use `Arc<Mutex<>>` with `PoisonError::into_inner()`.
- `AgentId` in `src/agent_id.rs` (breaks `agent.rs` ↔ `registry.rs` circular import).
- `reset()` must call `idle_notify.notify_waiters()` after clearing `loop_active`.

## Lifecycle
- `pause()` must snapshot full in-flight message history (not LLM-only) and loop-local `pending_messages`.
- Checkpoint restore validates `session_state` before mutating; rejects active runs with `WouldBlock`.
- `continue_*()` from assistant tail must drain queued steering/follow-up before first new turn.
- `new_blocking_runtime()` propagates failures as `AgentError::RuntimeInit`.
