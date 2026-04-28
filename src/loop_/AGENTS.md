# Loop Lessons

## Structure
- Nested outer/inner loop: outer = multi-turn follow-up, inner = single turn.
- `overflow_signal` on `LoopState` (not `AgentContext`), resets after `transform_context`. `transform_context` is synchronous.
- Tool dispatch order: PreDispatch policies → Approval → Schema validation → `execute()`.
- `turn_index` advances after every completed turn (including text-only).

## Tool Dispatch
- Pre-dispatch is batch-wide and two-pass: approval starts only after entire batch clears. `PreDispatchVerdict::Stop` synthesizes terminal errors for all unresolved calls and is terminal for the turn.
- `ApprovedWith(...)` rewrites re-enter pre-dispatch before enqueue.
- `ToolExecutionStrategy::partition()` indices target post-preprocessing `tool_calls`, not original list.
- Tool update events need call `id`/`name` via awaited relay (bounded, overflow coalesces). Post-turn replacements preserve original `ToolCall` blocks.
- Pre-dispatch snapshots `SessionState` once per batch, not per tool.

## Cancellation & Error
- Check cancellation in preprocessing and before each dispatch, not just at handle collection. Parent cancellation observed during handle collection via `select!`.
- Panicked tool task → synthesize error result + `ToolExecutionEnd`. Aborted batch terminates turn inline (no `handle_cancellation()`).
- Overflow recovery reuses started-turn cancellation path. `handle_stream_error()` must not emit `MessageEnd` for overflow.
- Provider-terminal `Error`/`Aborted` must commit assistant message and run post-turn policies.

## Steering & Turn Lifecycle
- Steering interrupt: if a worker drains steering, hand drained messages to shared batch state.
- Text-only turns poll steering after `TurnEnd`. `pending_messages` must drain before `BreakInner`.
- `TurnPolicyContext.context_messages` always a committed snapshot. `PreTurn` policies run after context transformers.
- Dynamic system prompts computed once per turn, carried into overflow retries.
- Retry attempts share one assistant-message lifecycle. Cache-prefix tracking is turn-to-turn.

## Approval
- Async approval needs `catch_unwind` on both callback invocation and future polling.
