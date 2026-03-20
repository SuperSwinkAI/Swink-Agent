# Feature Specification: Context Management

**Feature Branch**: `006-context-management`
**Created**: 2026-03-20
**Status**: Draft
**Input**: Context windowing, transformation hooks, versioned history, and message conversion pipeline. Manages how conversation history is pruned, transformed, and prepared for LLM providers. References: PRD §5 (Agent Context), PRD §10.1 (Context Window Overflow), PRD §12.2 (Loop Config — transform_context, convert_to_llm), HLD Agent Context.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Automatic Context Pruning for Long Conversations (Priority: P1)

A developer runs a long conversation that grows beyond the model's context window. The system automatically prunes the message history using a sliding window strategy: it preserves an anchor set of early messages and a tail of recent messages, removing middle messages to fit within the budget. Tool-result pairs are kept together even if this slightly exceeds the budget.

**Why this priority**: Without context pruning, long conversations fail with context overflow errors. This is the primary mechanism for keeping conversations alive.

**Independent Test**: Can be tested by creating a message history that exceeds a token budget and verifying the pruning preserves anchor and tail while removing the middle, with tool-result pairs intact.

**Acceptance Scenarios**:

1. **Given** a conversation exceeding the token budget, **When** the sliding window is applied, **Then** anchor messages (first N) and tail messages (most recent) are preserved.
2. **Given** a conversation with tool call/result pairs in the middle, **When** pruning occurs, **Then** tool-result pairs are kept together — a tool result is never separated from its tool call.
3. **Given** a conversation within the token budget, **When** the sliding window is applied, **Then** no messages are removed.

---

### User Story 2 - Custom Context Transformation (Priority: P1)

A developer provides a custom transformation hook that runs before each LLM call. This hook can inject context (e.g., retrieved documents), prune messages, or apply any custom logic. Both synchronous and asynchronous variants are supported. When context overflow occurs, the hook receives an overflow signal so it can apply more aggressive pruning on retry.

**Why this priority**: Custom transformation is the extensibility point for advanced context management — RAG injection, summarization, custom pruning strategies. It's called on every turn.

**Independent Test**: Can be tested by providing a transformation hook that modifies the message list and verifying the modified context reaches the provider.

**Acceptance Scenarios**:

1. **Given** a synchronous transformation hook, **When** a turn begins, **Then** the hook is called with the current context before the conversion pipeline.
2. **Given** an asynchronous transformation hook, **When** a turn begins, **Then** the hook is awaited with the current context.
3. **Given** a context overflow on the previous attempt, **When** the transformation hook is called on retry, **Then** it receives the overflow signal.
4. **Given** no transformation hook configured, **When** a turn begins, **Then** the context passes through unchanged.

---

### User Story 3 - Message Conversion Pipeline (Priority: P1)

When preparing context for the LLM provider, the system converts each agent message to the provider's expected format. A conversion function maps each message, returning the converted form or nothing to filter it out. Custom application-defined messages are filtered out by default since they should never reach the provider.

**Why this priority**: The conversion pipeline is how custom and non-LLM messages are excluded from provider input. Without it, custom messages would cause provider errors.

**Independent Test**: Can be tested by creating a message history with standard and custom messages, applying the conversion, and verifying custom messages are filtered out.

**Acceptance Scenarios**:

1. **Given** a message history with standard messages, **When** the conversion function runs, **Then** each message is converted to the provider format.
2. **Given** a message history with custom application messages, **When** the conversion function runs, **Then** custom messages are filtered out (return nothing).
3. **Given** the conversion function, **When** it is called, **Then** it runs after the transformation hook on every turn.

---

### User Story 4 - Versioned Context History (Priority: P3)

A developer needs to track how the context evolves across turns for debugging or analysis. The system maintains versioned snapshots of the context, allowing inspection of what the agent saw at each turn boundary.

**Why this priority**: Context versioning is a debugging/observability feature — useful but not required for the agent to function.

**Independent Test**: Can be tested by running a multi-turn conversation and verifying that each turn's context snapshot can be retrieved.

**Acceptance Scenarios**:

1. **Given** a multi-turn conversation, **When** the context is versioned, **Then** each turn's context snapshot is independently retrievable.
2. **Given** context versions, **When** they are inspected, **Then** they show the progression of messages, transformations, and pruning across turns.

---

### Edge Cases

- What happens when the token budget is smaller than the anchor + one recent message — does the system preserve minimum viable context?
- How does the system estimate tokens for custom messages that have no text content?
- What happens when the transformation hook adds messages that push the context over budget — is the sliding window reapplied?
- How does the system handle an empty conversation history — does it pass an empty list to the provider?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST provide a sliding window context pruning strategy that preserves an anchor set (first N messages) and a tail set (most recent messages), removing middle messages to fit a token budget.
- **FR-002**: Sliding window MUST preserve tool call/result pairs together — a tool result MUST NOT be separated from its corresponding tool call, even if this slightly exceeds the budget.
- **FR-003**: System MUST provide a token estimation heuristic for determining message sizes.
- **FR-004**: Custom messages MUST be estimated at a flat token cost since they have no standard text content.
- **FR-005**: System MUST support a synchronous context transformation hook called before each provider call.
- **FR-006**: System MUST support an asynchronous context transformation hook as an alternative to the synchronous variant.
- **FR-007**: The transformation hook MUST receive an overflow signal when called after a context overflow error, enabling more aggressive pruning on retry.
- **FR-008**: System MUST provide a message conversion function that maps each agent message to the provider format, with the ability to filter messages by returning nothing.
- **FR-009**: The conversion function MUST run after the transformation hook on every turn.
- **FR-010**: System MUST support versioned context history that tracks context snapshots at turn boundaries.

### Key Entities

- **SlidingWindow**: Context pruning strategy — anchor set + tail set, middle removed, tool-result pairs preserved.
- **ContextTransformer**: Synchronous hook for rewriting context before each provider call.
- **AsyncContextTransformer**: Asynchronous variant of the context transformation hook.
- **ContextVersion**: Versioned snapshot of the context at a turn boundary.
- **ConvertToLlmFn**: Function that maps an agent message to a provider message or filters it out.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Sliding window pruning correctly preserves anchor and tail messages while removing middle messages to fit within the budget.
- **SC-002**: Tool call/result pairs are never separated by pruning.
- **SC-003**: The transformation hook is called before the conversion function on every turn, in both sync and async variants.
- **SC-004**: The overflow signal is correctly propagated to the transformation hook after a context overflow error.
- **SC-005**: Custom messages are filtered out by the conversion pipeline and never reach the provider.
- **SC-006**: Token estimation produces consistent results for the same message content.

## Assumptions

- Token estimation uses a characters-divided-by-4 heuristic as an approximation. Exact tokenization is not required.
- Custom messages are estimated at 100 tokens flat.
- The sliding window anchor size and tail size are configurable by the caller.
- Context transformation is synchronous by default; the async variant is an opt-in alternative.
- Versioned context history is opt-in and does not impose overhead when not used.
