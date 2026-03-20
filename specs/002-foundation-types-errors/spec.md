# Feature Specification: Foundation Types & Errors

**Feature Branch**: `002-foundation-types-errors`
**Created**: 2026-03-20
**Status**: Draft
**Input**: Core data model types and error taxonomy for the agent harness. All types that every other module depends on. References: PRD §3 (Core Data Model), PRD §10.3 (AgentError Variants), HLD Foundation Layer.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Define Message Types for Conversation History (Priority: P1)

A developer building an agent application needs to represent conversation history as a sequence of typed messages. The system provides distinct message types for user input, assistant responses, and tool results, each carrying role-appropriate content. Messages compose into a conversation that can be passed to any LLM provider without loss of information.

**Why this priority**: Messages are the fundamental data unit of the entire system. Every other component — the loop, tools, streaming, adapters — operates on messages. Without a correct, complete message type hierarchy, nothing else can be built.

**Independent Test**: Can be tested by constructing each message type, composing them into a conversation sequence, and verifying that all fields are accessible, content blocks are correctly typed, and round-trip serialization preserves all data.

**Acceptance Scenarios**:

1. **Given** the type system, **When** a developer creates a user message with text content, **Then** the message carries the correct role, content blocks, and timestamp.
2. **Given** the type system, **When** a developer creates an assistant message, **Then** it carries content blocks, provider identifier, model identifier, usage statistics, stop reason, and optional error message.
3. **Given** the type system, **When** a developer creates a tool result message, **Then** it carries the tool call ID, content blocks, and an error flag.
4. **Given** any message type, **When** it is serialized and deserialized, **Then** all fields round-trip correctly with no data loss.

---

### User Story 2 - Represent Rich Content Blocks (Priority: P1)

A developer needs to compose message content from multiple block types: plain text, reasoning/thinking traces, tool call invocations, and images. Each block type carries its own structure, and a single message may contain multiple blocks of different types.

**Why this priority**: Content blocks are the atomic unit of message content. Tool calls, thinking traces, and multi-modal content all depend on a correct content block representation.

**Independent Test**: Can be tested by constructing content blocks of each type, embedding them in messages, and verifying correct access and pattern matching.

**Acceptance Scenarios**:

1. **Given** the content block types, **When** a text block is created, **Then** it contains a plain text string.
2. **Given** the content block types, **When** a thinking block is created, **Then** it contains a reasoning string and an optional verification signature.
3. **Given** the content block types, **When** a tool call block is created, **Then** it contains a call ID, tool name, parsed arguments, and an optional partial argument buffer for streaming.
4. **Given** the content block types, **When** an image block is created, **Then** it contains image data from a supported source type.

---

### User Story 3 - Track Token Usage and Cost (Priority: P1)

An application operator needs to monitor resource consumption across agent runs. Every assistant response carries token usage counters (input, output, cache read, cache write, total) and a cost breakdown (per-category and total) so the operator can track spending and optimize model selection.

**Why this priority**: Usage and cost tracking is essential for production deployments where budget constraints and cost visibility are non-negotiable.

**Independent Test**: Can be tested by creating usage and cost records, aggregating them across multiple responses, and verifying correct arithmetic.

**Acceptance Scenarios**:

1. **Given** a usage record, **When** token counts are set, **Then** all counters (input, output, cache read, cache write, total) are independently accessible.
2. **Given** two usage records, **When** they are aggregated, **Then** each counter sums correctly.
3. **Given** a cost record, **When** per-category costs are set, **Then** the total is the sum of all categories.

---

### User Story 4 - Handle Errors as Typed Conditions (Priority: P1)

A developer handling failures from the agent needs each error condition to be a distinct, matchable type with a meaningful description. The error taxonomy covers all failure modes: context overflow, rate limiting, network errors, structured output failures, concurrency violations, stream errors, and cancellation.

**Why this priority**: Typed errors enable callers to handle each failure mode appropriately (retry vs abort vs resize context). A flat error string would make robust error handling impossible.

**Independent Test**: Can be tested by constructing each error variant and verifying it carries the expected context, displays a meaningful message, and implements the standard error trait.

**Acceptance Scenarios**:

1. **Given** each error variant, **When** it is constructed, **Then** it carries the correct contextual data (e.g., model name for context overflow, attempt count for structured output failure).
2. **Given** any error, **When** it is displayed, **Then** it produces a human-readable message describing the failure.
3. **Given** any error, **When** it is used in standard error handling, **Then** it implements the standard error trait and supports error chaining.

---

### User Story 5 - Extend Messages with Application-Specific Types (Priority: P2)

An application developer needs to attach custom message types (e.g., notifications, artifacts, UI events) to the conversation history without modifying the core message types. The system provides an open extension point where any application-defined type that meets basic thread-safety requirements can be wrapped alongside standard messages.

**Why this priority**: Extensibility via custom messages is important for real-world applications but is secondary to the core message types working correctly.

**Independent Test**: Can be tested by defining a custom message type, wrapping it in the extension mechanism, and verifying it can be stored in conversation history and downcast back to its original type.

**Acceptance Scenarios**:

1. **Given** an application-defined custom type, **When** it is wrapped as an agent message, **Then** it can be stored in conversation history alongside standard messages.
2. **Given** a custom message in conversation history, **When** it is accessed, **Then** it can be downcast back to its original type for application-specific processing.
3. **Given** a custom message, **When** the conversation history is passed to a provider, **Then** custom messages are filtered out by the conversion pipeline and never reach the LLM.

---

### Edge Cases

- What happens when a usage record has all zero counters — is this valid or treated as empty?
- How does the system handle a content block with an empty text string?
- What happens when a tool call block has an empty argument object — is it treated as valid empty arguments or an error?
- How does the system behave when a custom message type fails to downcast — does it return a typed failure or a silent None?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST define a content block type with variants for text, thinking, tool call, and image.
- **FR-002**: System MUST define three message types: user message, assistant message, and tool result message, each with role-appropriate fields.
- **FR-003**: System MUST define a wrapper type that can hold either a standard message or an application-defined custom message.
- **FR-004**: Custom message types MUST satisfy thread-safety requirements and support runtime type identification for downcasting.
- **FR-005**: System MUST define a usage type with counters for input tokens, output tokens, cache read tokens, cache write tokens, and total tokens.
- **FR-006**: System MUST define a cost type with per-category costs and a total cost.
- **FR-007**: Usage and cost types MUST support aggregation (summing across multiple records).
- **FR-008**: System MUST define a stop reason type indicating why generation ended: natural stop, length limit, tool use requested, aborted, or error.
- **FR-009**: System MUST define a model specification type carrying provider identifier, model identifier, reasoning depth level, and optional per-level token budget overrides.
- **FR-010**: System MUST define a reasoning depth type with levels: off, minimal, low, medium, high, and extra-high.
- **FR-011**: System MUST define an agent result type carrying produced messages, final stop reason, aggregated usage, and optional error.
- **FR-012**: System MUST define an agent context type carrying system prompt, message history, and available tools.
- **FR-013**: System MUST define an error type with distinct variants for: context window overflow (with model name), rate limiting, network error, structured output failure (with attempt count and last error), already running, no messages, invalid continue, stream error (with source), and aborted.
- **FR-014**: All error variants MUST implement the standard error trait and provide meaningful display messages.
- **FR-015**: All public types MUST be safe to share across threads and safe to send between threads.
- **FR-016**: All types with data payloads MUST support serialization and deserialization.

### Key Entities

- **ContentBlock**: Atomic unit of message content — text, thinking, tool call, or image.
- **LlmMessage**: A standard conversation message (user, assistant, or tool result) that can be sent to an LLM provider.
- **AgentMessage**: Open wrapper holding either an LlmMessage or a custom application-defined message.
- **Usage**: Token consumption counters for a single LLM response.
- **Cost**: Financial cost breakdown for a single LLM response.
- **StopReason**: Why the LLM stopped generating (natural end, length limit, tool use, abort, error).
- **ModelSpec**: Target model configuration (provider, model ID, reasoning depth, token budgets).
- **AgentResult**: Outcome of a complete agent run (messages, stop reason, aggregated usage, error).
- **AgentContext**: Immutable snapshot of system prompt, messages, and tools passed into each loop turn.
- **AgentError**: Typed error taxonomy covering all failure conditions.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Every content block variant can be constructed, embedded in a message, and accessed without loss of data.
- **SC-002**: All three message types round-trip through serialization/deserialization with zero data loss.
- **SC-003**: Usage and cost aggregation produces arithmetically correct results across any number of records.
- **SC-004**: Every error variant constructs correctly, displays a meaningful message, and implements standard error handling.
- **SC-005**: All public types pass compile-time thread-safety verification.
- **SC-006**: Custom messages can be wrapped, stored, and downcast without modifying core types.
- **SC-007**: Model specification supports all six reasoning depth levels and optional per-level budget overrides.
- **SC-008**: Agent result correctly aggregates messages, stop reason, usage, and error from a multi-turn run.

## Assumptions

- Thread-safety requirements mean all public types must be Send and Sync.
- Serialization uses a standard format (JSON) but the spec does not prescribe the specific library.
- The CustomMessage extension point uses trait objects with runtime downcasting, not generics, to keep the API ergonomic.
- Token counts are unsigned integers; costs are floating-point values.
- Timestamps on messages use a standard representation but the spec does not prescribe the specific type.
