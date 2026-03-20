# Feature Specification: Integration Tests

**Feature Branch**: `030-integration-tests`
**Created**: 2026-03-20
**Status**: Draft
**Input**: End-to-end integration tests exercising the full stack (Agent, loop, mock stream, tool execution, events). Test infrastructure (MockStreamFn, MockTool, EventCollector, shared helpers). One test per PRD acceptance criterion (AC 1-30). References: PRD §17 (Acceptance Criteria AC 1-30), all architecture docs.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Verify Core Agent Lifecycle and Events (Priority: P1)

A library consumer needs confidence that the agent starts, processes messages, emits lifecycle events, and shuts down correctly. Integration tests exercise the full path: create an agent, attach a mock stream that returns scripted responses, subscribe to events, send a user message, and verify the correct sequence of lifecycle events is emitted (turn start, streaming, turn end). The tests run without any external services — all provider interaction is replaced by mock streams.

**Why this priority**: Lifecycle correctness is the foundation. If the agent cannot start, process a message, and emit events, nothing else works. This covers PRD acceptance criteria AC 1-5 (agent creation, message processing, event emission, streaming, turn completion).

**Independent Test**: Can be tested by creating an agent with a mock stream, sending a message, collecting events, and asserting the expected event sequence.

**Acceptance Scenarios**:

1. **Given** an agent with a mock stream, **When** a user message is sent, **Then** lifecycle events are emitted in order: turn start, content streaming, turn end.
2. **Given** an agent with event subscribers, **When** a turn completes, **Then** all subscribers receive the events.
3. **Given** an agent, **When** it is created with a system prompt, **Then** the system prompt is included in the first request to the stream.
4. **Given** a panicking event subscriber, **When** an event is dispatched, **Then** the subscriber is removed and other subscribers are unaffected.
5. **Given** an agent, **When** multiple turns are processed, **Then** conversation history accumulates correctly.

---

### User Story 2 - Verify Tool Execution and Validation (Priority: P1)

A library consumer needs confidence that tools are discovered, validated, executed, and that results flow back into the conversation. Integration tests register mock tools with configurable behavior (success, failure, latency), trigger tool calls via scripted stream responses, and verify: schema validation rejects malformed arguments, valid calls execute and return results, tool results are included in follow-up messages, concurrent tool calls execute in parallel, and tool errors are handled gracefully.

**Why this priority**: Tool execution is the agent's primary capability. This covers PRD acceptance criteria AC 6-12 (tool registration, schema validation, execution, concurrency, error handling, follow-up, tool call transformation).

**Independent Test**: Can be tested by registering mock tools, scripting a stream that returns tool calls, and asserting correct execution, validation, and result flow.

**Acceptance Scenarios**:

1. **Given** a registered tool, **When** the stream returns a tool call with valid arguments, **Then** the tool executes and the result is included in the next turn.
2. **Given** a registered tool, **When** the stream returns a tool call with invalid arguments, **Then** schema validation rejects the call and an error result is returned.
3. **Given** multiple tool calls in one turn, **When** the stream returns them, **Then** they execute concurrently (not sequentially).
4. **Given** a tool that fails, **When** it is called, **Then** the error result is returned to the agent and the loop continues.
5. **Given** a tool call transformer, **When** a tool call passes through, **Then** the transformer can modify the call before execution.
6. **Given** a tool validator, **When** a tool call passes through, **Then** the validator can reject the call before execution.

---

### User Story 3 - Verify Context Management and Overflow (Priority: P1)

A library consumer needs confidence that context management works correctly: the sliding window preserves recent messages when the context fills up, tool-result pairs stay together, and context overflow triggers the retry mechanism. Integration tests use mock streams with token counting to simulate context limits and verify compaction, overflow signaling, and recovery.

**Why this priority**: Context overflow is a silent failure mode if not handled — the agent stops working mid-conversation. This covers PRD acceptance criteria AC 13-16 (context window, sliding window, overflow, compaction).

**Independent Test**: Can be tested by configuring a small context budget, sending enough messages to exceed it, and verifying compaction preserves recent messages and overflow triggers retry.

**Acceptance Scenarios**:

1. **Given** a context budget, **When** messages exceed the budget, **Then** the sliding window removes middle messages while preserving anchor and tail.
2. **Given** a tool call and its result, **When** context compaction runs, **Then** the pair is kept or removed together (never split).
3. **Given** context overflow, **When** the overflow sentinel is detected, **Then** the loop retries with a compacted context.
4. **Given** a transform_context callback, **When** overflow occurs, **Then** the callback is invoked to allow custom compaction.

---

### User Story 4 - Verify Retry, Steering, and Abort (Priority: P2)

A library consumer needs confidence in the agent's resilience and controllability. Integration tests verify: retry logic handles throttling and network errors with backoff, steering callbacks can modify messages between turns, the abort mechanism stops the agent mid-turn, and the synchronous API wrapper works correctly. These tests use mock streams that simulate error conditions and controllable timing.

**Why this priority**: Retry, steering, and abort are important for production resilience but are secondary to basic lifecycle and tool execution. This covers PRD acceptance criteria AC 17-22 (retry, steering, abort, sync API, follow-up decisions, custom messages).

**Independent Test**: Can be tested by configuring mock streams to return errors, attaching steering callbacks, triggering abort signals, and asserting correct behavior.

**Acceptance Scenarios**:

1. **Given** a mock stream that returns a throttle error, **When** retry is configured, **Then** the agent retries with backoff and eventually succeeds.
2. **Given** a steering callback, **When** a turn completes, **Then** the callback can inject or modify messages before the next turn.
3. **Given** a running agent turn, **When** abort is signaled, **Then** the turn stops and the agent returns to idle.
4. **Given** the synchronous API, **When** a message is sent, **Then** it blocks until the response is complete and returns the result.
5. **Given** a follow-up decision callback, **When** a turn completes, **Then** the callback determines whether the agent continues with another turn.
6. **Given** a custom message type, **When** included in context, **Then** it survives compaction but is not sent to the provider.

---

### User Story 5 - Verify Structured Output and Proxy Reconstruction (Priority: P2)

A library consumer needs confidence that structured output mode and proxy stream reconstruction work correctly. Integration tests verify: the agent can request structured output with a schema and receive validated responses, and proxy streams can reconstruct complete event sequences from serialized data. These tests use mock streams that return structured responses and serialized event data.

**Why this priority**: Structured output and proxy reconstruction are advanced features used in specific workflows. This covers PRD acceptance criteria AC 23-25 (structured output, schema enforcement, proxy reconstruction).

**Independent Test**: Can be tested by configuring structured output with a schema, sending a message, and asserting the response conforms to the schema.

**Acceptance Scenarios**:

1. **Given** structured output mode with a schema, **When** the agent receives a response, **Then** the response conforms to the specified schema.
2. **Given** a proxy stream with serialized events, **When** events are reconstructed, **Then** the full event sequence is faithfully reproduced.
3. **Given** structured output with an invalid response, **When** schema validation fails, **Then** an appropriate error is returned.

---

### User Story 6 - Verify TUI Rendering and Interaction (Priority: P3)

A library consumer needs confidence that TUI components render correctly with agent data. Integration tests verify: conversation messages render with correct role colors, inline diffs display additions and removals correctly, the context gauge reflects utilization percentages, tool blocks show spinners and badges, and the external editor integration round-trips content. These tests use the TUI's rendering components in a test harness without a live terminal.

**Why this priority**: TUI tests are important for preventing visual regressions but are lower priority than core agent behavior. This covers PRD acceptance criteria AC 26-30 (TUI rendering, diffs, context gauge, editor, plan mode, approval).

**Independent Test**: Can be tested by rendering TUI components with known data and asserting the output contains expected styled elements.

**Acceptance Scenarios**:

1. **Given** a conversation with mixed message roles, **When** rendered, **Then** each role has its designated border color.
2. **Given** a file diff, **When** rendered inline, **Then** additions are green, removals are red, and context is dimmed.
3. **Given** context utilization at 70%, **When** the gauge renders, **Then** it displays in yellow.
4. **Given** plan mode is active, **When** the UI renders, **Then** the plan mode indicator is visible and write tools are unavailable.
5. **Given** Smart approval mode, **When** a read tool is called, **Then** it auto-executes; when a write tool is called, an approval prompt is shown.

---

### Edge Cases

- What happens when a mock stream emits events out of order (e.g., content before start)?
- How do tests handle non-deterministic timing in concurrent tool execution assertions?
- What happens when a test registers the same tool name twice?
- How do tests verify retry backoff timing without making tests slow?
- What happens when a mock tool panics during execution?
- How do tests ensure event subscriber assertions are complete (no missed events)?
- What happens when structured output schema validation encounters an edge case (empty object, deeply nested)?
- How do tests handle platform-specific differences in clipboard or editor behavior?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The test suite MUST include a mock stream component that returns scripted event sequences without contacting external services.
- **FR-002**: The test suite MUST include a mock tool component with configurable behavior: success results, error results, configurable latency, and failure modes.
- **FR-003**: The test suite MUST include an event collector that subscribes to agent events and stores them for assertion.
- **FR-004**: Shared test helpers MUST be organized in a common module reusable across all test files.
- **FR-005**: The test suite MUST include at least one test for each PRD acceptance criterion (AC 1 through AC 30).
- **FR-006**: All tests MUST run without external services, network access, or API keys.
- **FR-007**: Tests MUST verify the agent lifecycle event sequence: turn start, content streaming, turn end.
- **FR-008**: Tests MUST verify tool schema validation rejects malformed arguments.
- **FR-009**: Tests MUST verify concurrent tool execution (multiple tools in one turn run in parallel).
- **FR-010**: Tests MUST verify the sliding window context compaction preserves anchor and tail messages.
- **FR-011**: Tests MUST verify context overflow triggers the retry mechanism via the overflow sentinel.
- **FR-012**: Tests MUST verify retry logic with configurable backoff for throttle and network errors.
- **FR-013**: Tests MUST verify steering callbacks can modify messages between turns.
- **FR-014**: Tests MUST verify the abort mechanism stops a running turn.
- **FR-015**: Tests MUST verify structured output schema enforcement.
- **FR-016**: Tests MUST verify proxy stream event reconstruction.
- **FR-017**: Tests MUST verify TUI component rendering with correct role-based styling.
- **FR-018**: Tests MUST verify inline diff rendering with correct color coding.
- **FR-019**: Tests MUST verify the context gauge color thresholds.
- **FR-020**: Tests MUST verify plan mode restricts write tools and approval modes classify tools correctly.

### Key Entities

- **MockStreamFn**: A configurable mock that replaces the real stream provider, returning scripted event sequences (text tokens, tool calls, errors) in a deterministic order.
- **MockTool**: A controllable mock tool with configurable return values, latency, and failure modes. Used to test tool execution, validation, and error handling.
- **EventCollector**: A subscriber that captures all agent events into an ordered collection for post-hoc assertion. Supports filtering by event type.
- **TestHelpers**: Shared utility functions in a common module providing convenience constructors for agents, mock streams, mock tools, and assertion helpers.
- **AcceptanceCriterion**: A mapping from each PRD acceptance criterion (AC 1-30) to one or more integration tests that verify it.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Every PRD acceptance criterion (AC 1-30) has at least one passing integration test.
- **SC-002**: The entire integration test suite passes with zero external dependencies (no network, no API keys).
- **SC-003**: All tests run to completion within the CI timeout (each individual test completes in under 10 seconds).
- **SC-004**: The mock stream, mock tool, and event collector are reusable — at least 80% of tests share common helper infrastructure.
- **SC-005**: Adding a new test for a future acceptance criterion requires only writing the test, not new infrastructure.
- **SC-006**: No test depends on execution order — all tests are independent and can run in any order or in parallel.

## Assumptions

- All core library features (agent, loop, tools, context, streaming, adapters) from specs 001-024 are implemented and have unit tests.
- TUI components from specs 025-029 expose rendering functions that can be tested in a headless mode (no live terminal required).
- The PRD defines exactly 30 acceptance criteria (AC 1-30) that map to testable behaviors.
- Mock streams can simulate any provider behavior including errors, throttling, and structured output.
- The test suite runs as part of the standard workspace test command and does not require special setup.
- Concurrent tool execution tests use timing-independent assertions (e.g., verifying parallel start times) rather than wall-clock duration.
