# Feature Specification: Adapter: Ollama

**Feature Branch**: `014-adapter-ollama`
**Created**: 2026-03-20
**Status**: Draft
**Input**: OllamaStreamFn for /api/chat via NDJSON streaming (NOT SSE). Native tool-calling protocol support. Ollama message conversion. References: PRD §15.1, HLD Adapters.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Stream Text Responses from Ollama (Priority: P1)

A developer configures the Ollama adapter with a base URL (defaulting to localhost) and model name and sends a conversation to the Ollama chat endpoint. The adapter streams back text content in real time as newline-delimited JSON, delivering each text delta to the agent loop as it arrives. The developer sees assistant responses appear incrementally from their local model.

**Why this priority**: Streaming text is the fundamental capability — without it, the adapter has no value.

**Independent Test**: Can be tested by sending a simple prompt to a running Ollama instance and verifying that text deltas arrive incrementally and the final assembled message is coherent.

**Acceptance Scenarios**:

1. **Given** a running Ollama instance and a model name, **When** a conversation is sent, **Then** text content streams back incrementally via newline-delimited JSON.
2. **Given** a streaming response, **When** all lines have arrived, **Then** the assembled message matches what the model produced.
3. **Given** a streaming response, **When** the final JSON line indicates completion (`done: true`), **Then** a terminal event signals completion.

---

### User Story 2 - Stream Tool Call Responses from Ollama (Priority: P1)

A developer sends a conversation that includes tool definitions to the Ollama chat endpoint. The adapter receives tool call responses via Ollama's native tool-calling protocol and emits structured tool call events that the agent loop can dispatch for execution.

**Why this priority**: Tool calling is essential for agentic workflows — the primary use case of this library.

**Independent Test**: Can be tested by sending a prompt with tool definitions to a model that supports tool calling, and verifying that tool call events arrive with correct names and parseable arguments.

**Acceptance Scenarios**:

1. **Given** a conversation with tool definitions, **When** the model decides to call a tool, **Then** the adapter emits tool call events with correct tool name and arguments.
2. **Given** tool call arguments from Ollama, **When** the tool call is complete, **Then** the arguments form valid JSON.
3. **Given** a response with multiple tool calls, **When** received, **Then** each tool call is emitted as a separate indexed block.

---

### User Story 3 - Consume NDJSON Streaming Protocol (Priority: P2)

A developer uses the Ollama adapter, which consumes newline-delimited JSON (NDJSON) instead of Server-Sent Events. Each line of the response body is a complete JSON object. The adapter parses this protocol correctly, handling partial lines, connection interruptions, and the done flag that signals stream completion. This is transparent to the developer — they receive the same event types as from any other adapter.

**Why this priority**: NDJSON parsing is what distinguishes this adapter from SSE-based adapters, but the developer experience is the same regardless of protocol.

**Independent Test**: Can be tested by feeding raw NDJSON lines to the parser and verifying correctly parsed events are produced, including handling of partial lines and the done flag.

**Acceptance Scenarios**:

1. **Given** a stream of NDJSON lines, **When** parsed, **Then** each line produces a correctly structured event.
2. **Given** an NDJSON line with `done: true`, **When** parsed, **Then** the stream is signaled as complete.
3. **Given** a partial line (incomplete JSON), **When** encountered, **Then** it is buffered until the line is complete rather than producing an error.

---

### User Story 4 - Handle Errors from Ollama (Priority: P2)

A developer encounters various error conditions when communicating with the Ollama instance (connection refused, model not found, out of memory, network timeout). The adapter classifies these errors using the shared error classifier so that the agent loop can apply appropriate retry strategies.

**Why this priority**: Correct error handling enables reliable operation, but the adapter can demonstrate value with the happy path alone.

**Independent Test**: Can be tested by simulating error responses and connection failures and verifying each maps to the correct error type.

**Acceptance Scenarios**:

1. **Given** a connection refused error (Ollama not running), **When** classified, **Then** it maps to a network error (retryable).
2. **Given** a model-not-found error, **When** classified, **Then** it maps to a non-retryable error.
3. **Given** a network timeout, **When** classified, **Then** it maps to a network error (retryable).

---

### Edge Cases

- What happens when the Ollama instance is not running — is the connection error surfaced clearly?
- How does the adapter handle a model that does not support tool calling when tools are provided?
- What happens when NDJSON lines arrive with unexpected fields or missing expected fields?
- How does the adapter handle an Ollama response that reports an error mid-stream (e.g., out of memory)?
- What happens when the model name does not exist on the Ollama instance?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The adapter MUST stream text responses from the Ollama chat endpoint via NDJSON, emitting incremental text deltas.
- **FR-002**: The adapter MUST stream tool call responses using Ollama's native tool-calling protocol.
- **FR-003**: The adapter MUST parse newline-delimited JSON, handling partial lines, the done flag, and mid-stream errors.
- **FR-004**: The adapter MUST convert agent messages to the Ollama message format using the shared conversion trait.
- **FR-005**: The adapter MUST classify errors using the shared error classifier (connection refused → network, model not found → non-retryable, timeout → network).
- **FR-006**: The adapter MUST default to localhost when no base URL is provided.

### Key Entities

- **OllamaStreamFn**: The streaming function that connects to the Ollama chat endpoint and produces assistant message events via NDJSON parsing.
- **NDJSON Parser**: Protocol handler that reads newline-delimited JSON lines and buffers partial lines.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Text responses stream incrementally — each NDJSON line produces a separate event, not buffered until completion.
- **SC-002**: Tool calls produce valid, parseable JSON arguments upon completion.
- **SC-003**: The NDJSON parser correctly handles partial lines, the done flag, and malformed lines without panics.
- **SC-004**: All Ollama error conditions map to the correct agent error types consistently.

## Assumptions

- Ollama uses newline-delimited JSON for streaming, not Server-Sent Events. The shared SSE parser is not used.
- Ollama runs locally by default (localhost) but may be configured to run on a remote host.
- Not all Ollama models support tool calling — the adapter handles this gracefully.
- The shared conversion trait and error classifier from the adapter shared infrastructure (spec 011) are available.
- The adapter does not manage model downloads — only communication with an already-running Ollama instance.
