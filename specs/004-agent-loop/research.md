# Research: Agent Loop

**Feature**: 004-agent-loop
**Date**: 2026-03-20 | **Updated**: 2026-03-31

## Technical Context Resolution

No NEEDS CLARIFICATION items. All decisions determined by the PRD (§12),
HLD execution layer, AGENTS.md lessons learned, and clarification session.

## Decisions

### Nested Loop Architecture

- **Decision**: Two nested loops — outer handles follow-up continuation,
  inner handles tool call cycles and steering.
- **Rationale**: PRD §12.3 specifies this architecture. The outer loop
  polls `get_follow_up_messages` only when the inner loop exits normally.
  Error/abort exits skip follow-up and exit immediately.
- **Alternatives considered**: Single flat loop with state machine —
  rejected because the nested structure naturally separates the two
  distinct continuation modes (tool-driven vs follow-up-driven).

### Concurrent Tool Execution

- **Decision**: `tokio::spawn` per tool call with per-tool child
  cancellation tokens.
- **Rationale**: Constitution principle III mandates efficiency;
  AGENTS.md explicitly calls out `tokio::spawn` for concurrent tool
  execution. Child cancellation tokens enable selective cancellation
  during steering interrupts.
- **Alternatives considered**: Sequential tool execution — rejected
  because it's the primary performance bottleneck. `join_all` without
  spawn — rejected because spawn gives true parallelism across the
  runtime's thread pool.

### Context Transformation Ordering

- **Decision**: `transform_context` (synchronous) runs before
  `convert_to_llm` on every turn.
- **Rationale**: AGENTS.md lessons learned explicitly state this order.
  Transform operates on `AgentMessage` values (pruning, budget),
  then convert filters to `LlmMessage` values for the provider.
  When no transform hook is provided, context passes through unchanged
  (identity behavior, per clarification).
- **Alternatives considered**: Async transform — rejected per AGENTS.md
  lesson learned ("transform_context is synchronous, not async").

### Overflow Signal Mechanism

- **Decision**: Boolean flag on `LoopState`, reset after
  `transform_context` processes it.
- **Rationale**: AGENTS.md lessons learned state "overflow_signal lives
  on LoopState, not AgentContext. Resets after transform_context."
  The sentinel `CONTEXT_OVERFLOW_SENTINEL` is a loop control signal,
  not an error that propagates to the caller.
- **Alternatives considered**: Typed overflow event — rejected because
  overflow is a retry control signal, not a lifecycle event.

### Steering Message Handling

- **Decision**: After each tool completion, poll `get_steering_messages`.
  If steering arrives: cancel remaining tools, inject error results for
  cancelled tools, process steering on next turn. When no tools are
  executing, steering messages are queued as pending for the next turn.
- **Rationale**: PRD §12.3 step 6 specifies this flow. Clarification
  confirmed queuing behavior when no tools are active.
- **Alternatives considered**: Interrupt provider call for steering —
  rejected because it adds complexity and the provider call is typically
  fast compared to tool execution.

### Event Emission Pattern

- **Decision**: Events emitted via an async channel (`tokio::mpsc`).
  The loop sends events; the caller receives them as an async stream.
- **Rationale**: Channel-based emission decouples the loop from
  consumers. The async stream return type matches FR-002.
- **Alternatives considered**: Callback-based emission — rejected
  because callbacks would require the loop to hold references to
  the subscriber, complicating lifetime management.

### Emergency Context Overflow Recovery (Updated 2026-03-31)

- **Decision**: When `ContextWindowExceeded` is detected in the stream
  error path, perform in-place recovery: set `overflow_signal = true`,
  re-run both async and sync context transformers, emit `ContextCompacted`
  events, and retry the LLM call within the same turn. Limited to one
  recovery attempt per turn via `overflow_recovery_attempted` guard.
- **Rationale**: The original design set `overflow_signal` and returned
  `TurnOutcome::ContinueInner`, which re-entered the turn from the top.
  This worked but had a subtle gap: the overflow error was surfaced as
  a `MessageEnd` event with the error *before* recovery, leaking the
  internal retry to event subscribers. In-place recovery keeps the
  overflow handling fully internal — subscribers only see the
  `ContextCompacted` event and the successful (or failed) result.
  AWS Strands uses the same pattern: `reduce_context()` is called
  in the exception handler, then the request is retried immediately.
- **Alternatives considered**: (1) Keep the sentinel-based approach —
  rejected because it leaks a `MessageEnd` error event before recovery
  and requires the turn to re-enter from the top, re-running policies
  and re-emitting `TurnStart`. (2) Add overflow as a `RetryStrategy`
  error kind — rejected because overflow recovery has fundamentally
  different semantics (compact + retry once, not exponential backoff).

### Max Tokens Recovery

- **Decision**: When `stop_reason: length` with incomplete tool calls,
  replace each incomplete tool call with an error result containing an
  informative message, then continue the loop.
- **Rationale**: PRD §10.2 specifies this as internal recovery — not
  surfaced as an AgentError. The agent sees error results and can
  adjust (e.g., make fewer tool calls).
- **Alternatives considered**: Surface as AgentError — rejected per
  PRD §10.2 ("purely internal recovery").
