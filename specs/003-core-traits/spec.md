# Feature Specification: Core Traits

**Feature Branch**: `003-core-traits`
**Created**: 2026-03-20
**Status**: Verified
**Input**: The three trait definitions that form the pluggable boundaries of the agent harness: tool execution, LLM streaming, and retry logic. Includes tool argument validation. References: PRD §4 (Tool System), PRD §7 (Streaming Interface), PRD §11 (Retry Strategy), HLD Core Abstractions layer.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Implement a Custom Tool (Priority: P1)

A developer creates a custom tool for their agent by implementing the tool trait. They define the tool's name, description, input schema, and async execution logic. When the agent invokes the tool, the harness validates the arguments against the declared schema before calling execute, and returns a structured result containing content blocks and optional details.

**Why this priority**: Tools are the primary mechanism for agents to take actions in the world. The tool trait is the contract that every tool implementation — builtin or custom — must satisfy.

**Independent Test**: Can be tested by implementing a mock tool, registering it, invoking it with valid and invalid arguments, and verifying validation and execution behavior.

**Acceptance Scenarios**:

1. **Given** a tool implementation, **When** it is invoked with arguments matching its schema, **Then** the execute function is called and returns a structured result.
2. **Given** a tool implementation, **When** it is invoked with arguments that violate its schema, **Then** the invocation is rejected with field-level validation errors without calling execute.
3. **Given** a tool implementation, **When** required fields are missing from the arguments, **Then** validation catches the missing fields.
4. **Given** a tool result, **When** it is examined, **Then** it contains content blocks (text or image) for the LLM and optional structured details for logging or display.

---

### User Story 2 - Plug in an LLM Provider (Priority: P1)

A developer connects their agent to an LLM provider by implementing the streaming interface. Their implementation accepts a model specification, agent context, and stream options, and returns an async stream of incremental events. The events follow a structured protocol: a start signal, content deltas (text, thinking, tool calls), and a terminal signal carrying usage statistics.

**Why this priority**: The streaming interface is the sole provider boundary. Without it, the agent cannot communicate with any LLM. Every adapter — cloud, local, proxy — implements this interface.

**Independent Test**: Can be tested by implementing a mock stream that emits a scripted event sequence and verifying the harness correctly accumulates the events into a finalized assistant message.

**Acceptance Scenarios**:

1. **Given** a streaming implementation, **When** it emits a sequence of text delta events, **Then** the delta accumulation logic assembles them into a complete text block.
2. **Given** a streaming implementation, **When** it emits interleaved text and tool call deltas, **Then** both blocks are correctly assembled with proper indexing.
3. **Given** a streaming implementation, **When** it emits a terminal done event with usage statistics, **Then** the finalized assistant message carries those statistics.
4. **Given** a streaming implementation, **When** it emits a terminal error event, **Then** the finalized assistant message carries the error and the appropriate stop reason.
5. **Given** stream options, **When** no overrides are specified, **Then** sensible defaults are used (no temperature, no max tokens, default transport).

---

### User Story 3 - Configure Retry Behavior for Transient Failures (Priority: P2)

An operator configures retry behavior for their agent so that transient failures (rate limits, network issues) are automatically retried while permanent failures fail immediately. The default retry behavior uses exponential backoff with jitter and a maximum attempt count. Advanced users can supply their own retry logic.

**Why this priority**: Retry strategy is essential for production reliability but is secondary to the core tool and streaming traits — the agent can function without retry, just less robustly.

**Independent Test**: Can be tested by simulating retryable and non-retryable errors and verifying the strategy makes correct retry/fail decisions and computes appropriate delays.

**Acceptance Scenarios**:

1. **Given** the default retry strategy, **When** a rate-limit error occurs on the first attempt, **Then** the strategy recommends retrying.
2. **Given** the default retry strategy, **When** a rate-limit error occurs on the maximum attempt, **Then** the strategy recommends not retrying.
3. **Given** the default retry strategy, **When** a context overflow error occurs, **Then** the strategy recommends not retrying (permanent failure).
4. **Given** the default retry strategy, **When** retry delays are computed, **Then** they increase exponentially and are capped at a maximum delay.
5. **Given** the default retry strategy with jitter enabled, **When** multiple delays are computed for the same attempt number, **Then** they vary within the jitter range.
6. **Given** a custom retry strategy, **When** it is provided to the agent, **Then** it replaces the default strategy for all retry decisions.

---

### Edge Cases

- What happens when a tool's parameter schema is empty (no parameters) — are empty arguments accepted?
- What happens when a streaming implementation emits events out of order (e.g., delta before start) — accumulation returns an error event and terminates the stream (strict enforcement).
- How does the system handle a tool call delta with an empty partial JSON string — is it treated as empty arguments `{}`?
- What happens when the retry strategy's delay exceeds the maximum cap — is it clamped?
- How does the system handle a streaming implementation that emits no events at all — treated as a stream error (no start event received).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST define a tool trait with methods for: unique name, human-readable label, natural-language description, input parameter schema, and async execute function.
- **FR-002**: The execute function MUST accept a tool call ID, validated parameters, a cancellation token, and an optional streaming update callback.
- **FR-003**: System MUST validate tool arguments against the declared parameter schema before calling execute. Invalid arguments MUST produce field-level validation errors without invoking execute.
- **FR-004**: Tool results MUST contain content blocks (for the LLM) and optional structured details (for logging/display), plus an error flag distinguishing success from failure.
- **FR-005**: Tool results MUST provide convenience constructors for error results (with error flag set) and success results (with error flag clear).
- **FR-006**: System MUST define a streaming interface trait that accepts a model specification, agent context, stream options, and cancellation token, and returns an async stream of assistant message events.
- **FR-007**: Stream options MUST support optional temperature, optional output token limit, optional session identifier, and transport preference.
- **FR-008**: Assistant message events MUST follow a start/delta/end protocol for each content type: text, thinking, and tool calls.
- **FR-009**: A terminal event (done or error) MUST close the stream and carry usage statistics.
- **FR-010**: System MUST provide delta accumulation logic that consumes an event stream and produces a finalized assistant message.
- **FR-011**: Delta accumulation MUST enforce strict ordering: one start, indexed content blocks, one terminal event.
- **FR-012**: Tool call partial JSON MUST be consumed on the tool call end event — parsed once. An empty string MUST be treated as empty arguments, not null.
- **FR-013**: System MUST define a retry strategy trait with methods for: should_retry (given error and attempt number) and delay (given attempt number).
- **FR-014**: System MUST provide a default retry strategy with configurable maximum attempts, base delay, maximum delay cap, exponential multiplier, and jitter toggle.
- **FR-015**: The default retry strategy MUST retry only rate-limit and network errors. It MUST NOT retry context overflow, abort, concurrency, structured output, or stream errors.
- **FR-016**: Jitter MUST vary the computed delay within a defined range around the exponential value.

### Key Entities

- **AgentTool**: The trait contract for tool implementations — name, description, schema, and async execute.
- **AgentToolResult**: Structured result of a tool execution — content blocks, details, and error flag.
- **StreamFn**: The trait contract for LLM provider streaming — the sole provider boundary.
- **StreamOptions**: Per-call configuration passed through to the provider (temperature, max tokens, session ID, transport).
- **AssistantMessageEvent**: Incremental streaming event (start, text/thinking/tool-call deltas, done/error terminal).
- **AssistantMessageDelta**: A single incremental update during streaming (text, thinking, or tool call fragment).
- **RetryStrategy**: The trait contract for retry decisions — should_retry and delay computation.
- **DefaultRetryStrategy**: Built-in retry with exponential backoff, jitter, max attempts, and max delay cap.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A mock tool can be implemented, have its arguments validated against its schema, and return a structured result — all without any LLM or network interaction.
- **SC-002**: A mock streaming implementation can emit a scripted event sequence that is correctly accumulated into a finalized assistant message with all content blocks intact.
- **SC-003**: The default retry strategy correctly distinguishes retryable errors (rate limit, network) from non-retryable errors (context overflow, abort, stream error) with 100% accuracy.
- **SC-004**: Exponential backoff delays increase correctly and never exceed the configured maximum cap.
- **SC-005**: Jitter produces varying delays for the same attempt number within the configured range.
- **SC-006**: Tool argument validation catches all schema violations (missing fields, wrong types, extra fields per schema rules) and reports field-level errors.
- **SC-007**: Delta accumulation handles interleaved text and tool call blocks in a single stream without data loss or ordering errors.

## Clarifications

### Session 2026-03-20

- Q: How should delta accumulation handle out-of-order events? → A: Return an error event and terminate the stream (strict enforcement).
- Q: What should happen when a stream emits zero events? → A: Treated as a stream error (no start event received).

## Assumptions

- The streaming protocol uses indexed content blocks so that multiple blocks can be streamed concurrently within a single message.
- The default retry jitter range is [0.5, 1.5) multiplied by the computed delay.
- The default retry strategy has 3 max attempts, 1-second base delay, and 60-second max delay cap.
- Tool argument validation uses JSON Schema (draft 7 or later) but the spec does not prescribe the specific validation library.
- The cancellation token mechanism uses cooperative cancellation, not forced abort.
