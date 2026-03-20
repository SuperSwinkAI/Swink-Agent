# Feature Specification: Agent Loop

**Feature Branch**: `004-agent-loop`
**Created**: 2026-03-20
**Status**: Draft
**Input**: The core execution engine for the agent harness. Implements the nested inner/outer loop, tool dispatch, steering/follow-up injection, event emission, retry integration, error/abort handling, and max tokens recovery. Stateless — all state is passed in via configuration and context. References: PRD §8 (Event System), PRD §9 (Cancellation), PRD §10.1-10.2 (Context Overflow, Max Tokens), PRD §12 (Agent Loop), HLD Execution Layer and Single Turn Data Flow.

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

### User Story 6 - Context Overflow Recovery (Priority: P2)

When the provider rejects a request because the context exceeds the model's window, the loop signals the overflow condition to the context transformation hook. On the next attempt, the hook applies more aggressive pruning, and the loop retries with the reduced context.

**Why this priority**: Context overflow is a common production issue with long conversations. Automatic recovery prevents hard failures.

**Independent Test**: Can be tested with a mock provider that rejects with overflow on the first call and succeeds on the second, verifying the overflow signal is passed to the transformation hook.

**Acceptance Scenarios**:

1. **Given** a context overflow error, **When** the loop retries, **Then** the transformation hook receives an overflow signal.
2. **Given** the transformation hook reduces context, **When** the provider is called again, **Then** the reduced context is used.

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
- How does the loop handle a provider that returns zero content blocks (empty response)?
- What happens when all tool calls in a batch are cancelled by steering — does the loop still emit turn end?
- What happens when the cancellation token is triggered during the provider call — does the loop emit an aborted stop reason?

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
- **FR-013**: Context overflow errors MUST signal the overflow condition to the transformation hook on retry.
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
- **SC-008**: Context overflow triggers the overflow signal to the transformation hook.
- **SC-009**: Incomplete tool calls from max tokens are replaced with error results and the loop continues.
- **SC-010**: Cancellation produces a clean shutdown with aborted stop reason.
- **SC-011**: The transformation hook is called before the convert-to-LLM function on every turn.

## Clarifications

### Session 2026-03-20

- Q: How should the loop behave when no context transformation hook is provided? → A: Use an identity transform — context passes through unchanged.
- Q: How should steering messages be handled when no tools are executing? → A: Queued as pending messages, processed before the next provider call.

## Assumptions

- The loop is stateless — all state is passed in via AgentLoopConfig and AgentContext. Mutable state lives in the caller (Agent struct).
- Concurrent tool execution uses task spawning with per-tool cancellation tokens.
- The overflow signal is a boolean flag on the loop state, not an event — it is reset after the transformation hook processes it.
- "Steering interrupt" means cancelling remaining tools and injecting error results; the steering message itself is processed on the next turn.
- The context overflow sentinel is a loop control signal, not an error that propagates to the caller.
