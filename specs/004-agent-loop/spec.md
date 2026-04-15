# Feature Specification: Agent Loop

**Feature Branch**: `004-agent-loop`
**Created**: 2026-03-20
**Updated**: 2026-03-31
**Status**: Draft
**Input**: The core execution engine for the agent harness. Implements the nested inner/outer loop, tool dispatch, steering/follow-up injection, event emission, retry integration, error/abort handling, emergency context overflow recovery, and max tokens recovery. Stateless — all state is passed in via configuration and context. References: PRD §8 (Event System), PRD §9 (Cancellation), PRD §10.1-10.2 (Context Overflow, Max Tokens), PRD §12 (Agent Loop), HLD Execution Layer and Single Turn Data Flow, AWS Strands conversation_manager.reduce_context() pattern.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Single-Turn Conversation (Priority: P1)

A developer sends a prompt to the agent and receives a streamed response. The loop calls the LLM provider, emits lifecycle events in the correct order (agent start, turn start, message start, message updates, message end, turn end, agent end), and produces a finalized assistant message. No tools are involved.

**Why this priority**: The most basic loop operation — a single prompt/response turn — must work correctly before any tool execution or multi-turn behavior.

**Independent Test**: Can be tested with a mock LLM provider that returns a scripted text response and verifying the event sequence matches the expected lifecycle.

**Acceptance Scenarios**:

1. **Given** a prompt and a mock provider, **When** the loop runs, **Then** events are emitted in order: AgentStart → TurnStart → MessageStart → MessageUpdate(s) → MessageEnd → TurnEnd → AgentEnd.
2. **Given** a single-turn conversation, **When** the loop completes, **Then** the agent end event carries the produced messages.
3. **Given** a single-turn conversation, **When** the provider returns a natural stop, **Then** the turn end event carries the assistant message with the correct stop reason.

---

### User Story 2 - Multi-Turn Tool Execution (Priority: P1)

The agent receives a response containing tool calls. The loop extracts the tool calls, executes them concurrently, collects results, injects them into context, and re-invokes the provider for a follow-up response. This continues until the provider stops requesting tools.

**Why this priority**: Agentic behavior — the defining feature — requires the loop to handle tool call → tool result → next turn cycles correctly.

**Independent Test**: Can be tested with a mock provider that requests tool calls on the first turn and returns a text response on the second, verifying both tool execution events and the multi-turn event sequence.

**Acceptance Scenarios**:

1. **Given** a provider response with tool calls, **When** the loop processes them, **Then** tool execution events are emitted (start, end) for each tool.
2. **Given** multiple tool calls in a single response, **When** they are executed, **Then** they run concurrently, not sequentially.
3. **Given** tool results, **When** they are injected into context, **Then** the provider is called again with the updated context.
4. **Given** a multi-turn conversation, **When** the provider stops requesting tools, **Then** the loop exits normally.

---

### User Story 3 - Steering Interrupts (Priority: P1)

During a long tool execution, the caller injects a steering message to redirect the agent. The loop detects the steering message after a tool completes, cancels remaining in-flight tools, injects error results for cancelled tools, and redirects the agent to process the steering message on the next turn.

**Why this priority**: Steering is essential for interactive agents where users need to redirect mid-execution. Without it, the user must wait for all tools to complete.

**Independent Test**: Can be tested with a mock provider and slow tools, injecting a steering message mid-execution and verifying that remaining tools are cancelled and the steering message is processed.

**Acceptance Scenarios**:

1. **Given** multiple concurrent tool executions, **When** a steering message arrives after one tool completes, **Then** remaining tools are cancelled via their cancellation tokens.
2. **Given** cancelled tools, **When** their results are injected, **Then** each gets an error result indicating cancellation due to steering interrupt.
3. **Given** a steering message, **When** it is injected, **Then** the next turn processes it before calling the provider.

---

### User Story 4 - Follow-Up Continuation (Priority: P2)

When the agent would normally stop (no more tool calls, natural end), the loop checks for follow-up messages. If follow-up messages are available, the loop continues with another turn using those messages instead of stopping.

**Why this priority**: Follow-up enables autonomous multi-step workflows where additional work is queued after the initial request completes.

**Independent Test**: Can be tested by providing a follow-up callback that returns messages on the first poll and nothing on the second, verifying the loop continues and then stops.

**Acceptance Scenarios**:

1. **Given** the loop has finished processing, **When** follow-up messages are available, **Then** the loop continues with another turn.
2. **Given** the loop has finished processing, **When** no follow-up messages are available, **Then** the loop emits AgentEnd and exits.
3. **Given** the loop ended with an error or abort, **When** follow-up polling would normally occur, **Then** no follow-up polling happens — the loop exits immediately.

---

### User Story 5 - Error Recovery and Retry (Priority: P2)

When the LLM provider returns a transient error (rate limit, network failure), the loop applies the retry strategy to determine whether to retry. If retryable, the loop waits for the computed delay and tries again. If the error is permanent or retries are exhausted, the loop surfaces the error and exits.

**Why this priority**: Production reliability requires automatic recovery from transient failures, but the loop must work correctly for the non-error case first.

**Independent Test**: Can be tested with a mock provider that fails on the first call and succeeds on the second, verifying the retry strategy is consulted and the loop recovers.

**Acceptance Scenarios**:

1. **Given** a retryable error, **When** the retry strategy approves retry, **Then** the loop waits for the computed delay and re-invokes the provider.
2. **Given** a retryable error, **When** retries are exhausted, **Then** the loop surfaces the error and exits.
3. **Given** a non-retryable error, **When** it occurs, **Then** the loop exits immediately without consulting the retry strategy.

---

### User Story 6 - Emergency Context Overflow Recovery (Priority: P2)

When the streaming layer returns `StreamErrorKind::ContextWindowExceeded`, the loop does NOT surface the error immediately. Instead, it performs emergency recovery: sets `overflow_signal = true`, re-runs the context transformation pipeline (both async and sync transformers) to compact the context, emits a `ContextCompacted` event, and retries the LLM call with the reduced context. If the retry also fails with overflow (compaction was insufficient), THEN the error is surfaced. This is a single-retry recovery — no infinite loops.

**Why this priority**: Context overflow is a common production issue with long conversations. The current implementation sets `overflow_signal` but only takes effect on the *next* turn — which never happens because the overflow error terminates the current turn. This change wires the overflow recovery directly into the stream error path so recovery happens in-place.

**Key reference**: AWS Strands' `conversation_manager.reduce_context()` which is called automatically when `ContextWindowOverflowException` is caught, then the request is retried.

**Independent Test**: Can be tested with a mock provider that rejects with overflow on the first call and succeeds on the second, verifying: (a) the transformation hook receives the overflow signal, (b) a `ContextCompacted` event is emitted, (c) the provider is retried with the reduced context, (d) if both attempts fail, the error is surfaced.

**Acceptance Scenarios**:

1. **Given** a `ContextWindowExceeded` error from the streaming layer, **When** the loop handles it, **Then** it does NOT surface the error immediately — it enters the emergency recovery path.
2. **Given** the emergency recovery path, **When** `overflow_signal` is set to true, **Then** both the async context transformer (if present) and the sync context transformer are re-run with `overflow=true`.
3. **Given** the transformers compact the context, **When** compaction produces a `CompactionReport`, **Then** a `ContextCompacted` event is emitted for each transformer that reports compaction.
4. **Given** the compacted context, **When** the LLM is retried, **Then** it uses the reduced context from the emergency compaction.
5. **Given** the retry also returns `ContextWindowExceeded`, **When** the second failure occurs, **Then** the error IS surfaced — no further retries (prevents infinite loops).
6. **Given** no context transformer is configured, **When** overflow occurs, **Then** the error is surfaced immediately (no compaction possible, no point retrying).
7. **Given** transformers are configured but neither reports compaction (both return `None`), **When** emergency recovery runs, **Then** the error is surfaced immediately without retrying the LLM call (compaction was ineffective, retry would hit the same overflow).
8. **Given** a multi-turn conversation, **When** a new turn starts, **Then** `overflow_recovery_attempted` resets to `false` — each turn gets its own independent recovery opportunity.

---

### User Story 7 - Max Tokens Recovery (Priority: P3)

When the provider stops mid-response because it hit the output token limit and there are incomplete tool calls, the loop replaces each incomplete tool call with an informative error result and continues the loop so the agent can try again.

**Why this priority**: This is a recovery mechanism for an uncommon but important edge case — truncated tool calls that would otherwise break the conversation.

**Independent Test**: Can be tested with a mock provider that returns a length-limited response with partial tool calls, verifying the loop injects error results and continues.

**Acceptance Scenarios**:

1. **Given** a response with stop reason "length" and incomplete tool calls, **When** the loop processes it, **Then** each incomplete tool call is replaced with an error result.
2. **Given** error results for incomplete tool calls, **When** the loop continues, **Then** the agent sees the error results and can adjust its approach.

---

### Edge Cases

- What happens when the context transformation hook is not provided — the loop uses an identity transform (context passes through unchanged).
- What happens when steering messages arrive but no tools are currently executing — they are queued as pending messages and processed before the next provider call.
- How does the loop handle a provider that returns zero content blocks (empty response)? → Treat as natural stop with empty assistant message; emit `TurnEnd` with `Complete` reason.
- What happens when all tool calls in a batch are cancelled by steering — does the loop still emit turn end? → Yes, emit `TurnEnd` with `SteeringInterrupt` reason.
- What happens when the cancellation token is triggered during the provider call — does the loop emit an aborted stop reason? → Yes, cancellation during provider call emits `TurnEnd` with `Aborted` reason followed by `AgentEnd`.
- What happens when overflow occurs but no context transformer is configured — the error is surfaced immediately. Without a transformer, compaction is not possible and retrying would produce the same result.
- Does emergency overflow recovery count as a retry for the RetryStrategy — no. Overflow recovery is a separate code path from the general retry strategy. The overflow recovery has its own single-retry limit (one compaction + one retry), independent of `RetryStrategy` attempts.
- Does emergency overflow recovery emit TurnStart/TurnEnd events for the retry — no. The recovery is internal to the stream error handling path. The retry re-runs the transform + stream within the same turn. Only the `ContextCompacted` event is emitted during recovery.
- What happens if the cancellation token fires during emergency overflow recovery — the loop checks cancellation between compaction and the retry stream call. If cancelled, recovery is aborted and the loop emits `Aborted` stop reason, consistent with existing cancellation semantics at turn boundaries.
- What if transformers run but neither reports compaction (both return `None`) — skip the retry and surface the error immediately. Compaction was ineffective; retrying would hit the same overflow.
- **Steering message preservation**: Tool-dispatch workers use a two-step pattern — `has_steering()` to check without draining, then a drain-and-handoff before setting the interrupt flag. This prevents steering messages already drained by a worker from being silently dropped. A second `poll_steering()` call cannot recover already-drained messages.
- **Three-phase tool dispatch**: Tool dispatch is split into three explicit phases: pre-process (argument transformation, policy evaluation, approval), execute (concurrent tool invocation), and collect (gathering results and emitting events). These correspond to separate modules in the implementation.
- **Approval callback panic isolation**: Approval futures are wrapped in `AssertUnwindSafe` + `catch_unwind`. A panicking approval callback is treated as a denial rather than crashing the loop.
- **Interrupt abort grace period**: After a steering interrupt is detected, tool tasks are given a 50 ms grace period (`INTERRUPT_ABORT_GRACE`) to finish before being forcibly aborted. This prevents indefinite hangs on blocking tools.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST provide two loop entry points: one that starts with new prompt messages and one that resumes from existing context.
- **FR-002**: Both entry points MUST return an async stream of lifecycle events.
- **FR-003**: The loop MUST emit lifecycle events in the correct order: AgentStart, then per-turn (TurnStart, MessageStart, MessageUpdate(s), MessageEnd, tool events if applicable, TurnEnd), then AgentEnd.
- **FR-004**: The loop MUST accept a configuration object carrying: model specification, stream options, retry strategy, convert-to-LLM function, and optional hooks for context transformation, API key resolution, steering messages, and follow-up messages.
- **FR-005**: The context transformation hook MUST be called before the convert-to-LLM function on every turn.
- **FR-006**: The API key resolution hook MUST be called before the provider on every turn to support dynamic/short-lived credentials.
- **FR-007**: Tool calls within a single turn MUST execute concurrently, not sequentially.
- **FR-008**: Each concurrent tool execution MUST receive its own child cancellation token.
- **FR-009**: After each tool completion, the loop MUST poll for steering messages. If steering arrives, remaining in-flight tools MUST be cancelled and error results injected for each cancelled tool.
- **FR-010**: When the inner loop exits normally, the outer loop MUST poll for follow-up messages. If available, the inner loop re-enters.
- **FR-011**: When the inner loop exits due to error or abort, the outer loop MUST NOT poll for follow-up — it MUST emit AgentEnd and exit immediately.
- **FR-012**: The loop MUST apply the retry strategy for retryable provider errors and respect the computed delay.
- **FR-013**: Context overflow errors MUST trigger emergency recovery: set `overflow_signal = true`, re-run both async and sync context transformers, emit `ContextCompacted` events, and retry the LLM call with the compacted context.
- **FR-013a**: Emergency overflow recovery MUST be limited to a single retry. If the retry also fails with overflow, the error MUST be surfaced.
- **FR-013b**: Emergency overflow recovery MUST NOT occur when no context transformer is configured — the error MUST be surfaced immediately.
- **FR-013c**: Emergency overflow recovery MUST be independent of the `RetryStrategy` — it has its own single-retry limit and does not consume retry attempts.
- **FR-013d**: Emergency overflow recovery MUST skip the retry if neither transformer reports compaction (both return `None`) — the error MUST be surfaced immediately.
- **FR-014**: When the provider returns stop reason "length" with incomplete tool calls, the loop MUST replace each incomplete tool call with an error result and continue.
- **FR-015**: Cancellation via the cancellation token MUST cause the loop to exit cleanly with an aborted stop reason.
- **FR-016**: The convert-to-LLM function MUST be able to filter messages by returning nothing for messages that should not reach the provider.

### Key Entities

- **AgentLoopConfig**: Configuration object carrying model, stream options, retry strategy, and all hook functions.
- **AgentEvent**: Lifecycle event enum with variants for agent, turn, message, and tool execution lifecycle.
- **LoopState**: Internal state tracking overflow signals, pending messages, and turn progression.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A single-turn no-tool conversation emits all lifecycle events in the correct order.
- **SC-002**: Tool calls within a single turn execute concurrently (verified by timing or execution order).
- **SC-003**: Steering messages interrupt tool execution — remaining tools are cancelled and error results are injected.
- **SC-004**: Follow-up messages cause the loop to continue after a natural stop.
- **SC-005**: Error or abort stop reasons cause immediate exit without follow-up polling.
- **SC-006**: Retryable errors trigger the retry strategy; the loop recovers on success.
- **SC-007**: Non-retryable errors cause immediate loop exit.
- **SC-008**: Context overflow triggers emergency recovery — transformers re-run with overflow=true, ContextCompacted emitted, LLM retried with compacted context. Second overflow surfaces the error.
- **SC-009**: Incomplete tool calls from max tokens are replaced with error results and the loop continues.
- **SC-010**: Cancellation produces a clean shutdown with aborted stop reason.
- **SC-011**: The transformation hook is called before the convert-to-LLM function on every turn.

## Clarifications

### Session 2026-03-20

- Q: How should the loop behave when no context transformation hook is provided? → A: Use an identity transform — context passes through unchanged.
- Q: How should steering messages be handled when no tools are executing? → A: Queued as pending messages, processed before the next provider call.

### Session 2026-03-31

- Q: Where does emergency overflow recovery execute — in the turn pipeline or the stream retry path? → A: In the stream error handling path (`handle_stream_result` or `stream_with_retry`). When `StreamResult::ContextOverflow` is returned, instead of encoding a sentinel and returning to the turn, the recovery re-runs transformers and retries the stream call in-place.
- Q: Does the CONTEXT_OVERFLOW_SENTINEL mechanism remain? → A: The sentinel encoding is replaced by in-place recovery. The `overflow_signal` flag on `LoopState` still exists but is set and consumed within the same turn rather than across turns.
- Q: How does this interact with the existing retry strategy? → A: They are independent. Overflow recovery has its own single-retry limit. A `ContextWindowExceeded` error does not consume `RetryStrategy` attempts. If overflow recovery succeeds, the turn continues normally. If it fails, the error is surfaced directly (not routed through `RetryStrategy`).
- Q: What if only the async transformer is configured (no sync)? → A: Only the async transformer runs. The recovery runs whatever transformers are available — async, sync, or both. If neither is configured, recovery is skipped and the error surfaces immediately.
- Q: What happens if the cancellation token fires during emergency overflow recovery? → A: The loop checks cancellation between compaction and the retry stream call. If cancelled, recovery aborts and emits `Aborted` stop reason. Consistent with existing cancellation semantics at turn boundaries.
- Q: What if transformers run but report no compaction (both return `None`)? → A: Skip the retry and surface the error immediately. If compaction was ineffective, retrying would hit the same overflow — avoids a wasted API call.

## Assumptions

- The loop is stateless — all state is passed in via AgentLoopConfig and AgentContext. Mutable state lives in the caller (Agent struct).
- Concurrent tool execution uses task spawning with per-tool cancellation tokens.
- The overflow signal is a boolean flag on the loop state, not an event — it is reset after the transformation hook processes it.
- "Steering interrupt" means cancelling remaining tools and injecting error results; the steering message itself is processed on the next turn.
- Emergency overflow recovery is bounded to one compaction + one retry per overflow occurrence. No infinite loops.
- The `CONTEXT_OVERFLOW_SENTINEL` mechanism is superseded by in-place recovery in the stream error path. The sentinel constant may remain for backward compatibility but is no longer the primary recovery path.
