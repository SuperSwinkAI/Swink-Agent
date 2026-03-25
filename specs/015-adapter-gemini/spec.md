# Feature Specification: Adapter: Google Gemini

**Feature Branch**: `015-adapter-gemini`
**Created**: 2026-03-20
**Status**: Draft
**Input**: GeminiStreamFn for Google Gemini API via SSE. Gemini-specific message and tool format conversion. References: PRD §15.1, HLD Adapters.

## Clarifications

### Session 2026-03-24

- Q: Should Gemini safety filter blocks / `"SAFETY"` finish reason emit an error or be treated as normal stop? → A: Emit `AssistantMessageEvent::error()`.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Stream Text Responses from Google Gemini (Priority: P1)

A developer configures the Gemini adapter with an API key and model selection and sends a conversation to the Gemini streaming endpoint. The adapter streams back text content in real time as Server-Sent Events, delivering each text delta to the agent loop as it arrives. The developer sees assistant responses appear incrementally.

**Why this priority**: Streaming text is the fundamental capability — without it, the adapter has no value.

**Independent Test**: Can be tested by sending a simple prompt to the Gemini endpoint and verifying that text deltas arrive incrementally and the final assembled message is coherent.

**Acceptance Scenarios**:

1. **Given** valid Gemini credentials and a model selection, **When** a conversation is sent, **Then** text content streams back incrementally via SSE.
2. **Given** a streaming response, **When** all deltas have arrived, **Then** the assembled message matches what the provider produced.
3. **Given** a streaming response, **When** the stream completes normally, **Then** a terminal event signals completion.

---

### User Story 2 - Stream Tool Call Responses from Google Gemini (Priority: P1)

A developer sends a conversation that includes tool definitions to the Gemini endpoint. The adapter converts tool definitions to Gemini's function declaration format, and streams back function call responses. The agent loop receives structured tool call events that it can dispatch for execution.

**Why this priority**: Tool calling is essential for agentic workflows — the primary use case of this library.

**Independent Test**: Can be tested by sending a prompt with tool definitions that the model is likely to invoke, and verifying that tool call events arrive with correct function names and parseable arguments.

**Acceptance Scenarios**:

1. **Given** a conversation with tool definitions, **When** the model decides to call a function, **Then** the adapter emits tool call events with correct function name and arguments.
2. **Given** tool call arguments from Gemini, **When** the tool call is complete, **Then** the arguments form valid JSON.
3. **Given** a response with multiple function calls, **When** streamed, **Then** each function call is emitted as a separate indexed block.

---

### User Story 3 - Convert Between Agent and Gemini Message Formats (Priority: P2)

A developer uses the Gemini adapter and the system handles conversion between the agent's message format and Gemini's distinct content structure. Gemini uses a different message schema than OpenAI-style APIs (parts-based content, function declarations instead of tools, function call/response instead of tool call/result). The adapter transparently handles these conversions so the developer works with a single consistent message format.

**Why this priority**: Format conversion is necessary for correctness but invisible to the developer — it enables the other stories.

**Independent Test**: Can be tested by converting agent messages to Gemini format and back, verifying all content types (text, tool calls, tool results, images) are preserved.

**Acceptance Scenarios**:

1. **Given** agent messages with various content types, **When** converted to Gemini format, **Then** text, tool calls, tool results, and images are all correctly represented.
2. **Given** Gemini's function declaration format, **When** tool definitions are converted, **Then** the schema and descriptions are preserved.
3. **Given** a Gemini function call response, **When** converted back, **Then** it produces standard tool call events.

---

### User Story 4 - Handle Errors from Google Gemini (Priority: P2)

A developer encounters various error conditions when communicating with the Gemini endpoint (invalid key, rate limiting, quota exceeded, server errors, network timeouts). The adapter classifies these errors using the shared error classifier so that the agent loop can apply appropriate retry strategies.

**Why this priority**: Correct error handling enables reliable operation, but the adapter can demonstrate value with the happy path alone.

**Independent Test**: Can be tested by simulating error responses (429, 401, 500, network timeout) and verifying each maps to the correct error type.

**Acceptance Scenarios**:

1. **Given** an HTTP 429 response from Gemini, **When** classified, **Then** it maps to a rate-limit error (retryable).
2. **Given** an HTTP 403 response (invalid API key or quota exceeded), **When** classified, **Then** it maps to an authentication error (not retryable).
3. **Given** a network timeout, **When** classified, **Then** it maps to a network error (retryable).
4. **Given** an HTTP 500 response, **When** classified, **Then** it maps to a network error (retryable).

---

### Edge Cases

- When Gemini returns a safety filter block or a finish reason of `"SAFETY"`, the adapter emits an `AssistantMessageEvent::error()` rather than silently dropping content or treating it as a normal stop.
- The adapter uses only the first candidate from multi-candidate responses; additional candidates are ignored.
- Image inputs are converted to Gemini's `inlineData` parts with the appropriate `mime_type`.
- Tool definition schemas are passed through to Gemini as-is; unsupported schema features (e.g., `oneOf`, `$ref`) are the caller's responsibility — the Gemini API will reject invalid schemas.
- When the conversation history contains thinking blocks, tool calls, or tool definitions, the adapter enables Gemini's thinking mode (`includeThoughts: true`) and streams thinking blocks as ThinkingStart/ThinkingDelta/ThinkingEnd events with `thoughtSignature`.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The adapter MUST stream text responses from the Gemini streaming endpoint via SSE, emitting incremental text deltas.
- **FR-002**: The adapter MUST stream function call responses, emitting function name, arguments, and completion events.
- **FR-003**: The adapter MUST convert agent tool definitions to Gemini's function declaration format.
- **FR-004**: The adapter MUST convert agent messages to Gemini's content format (parts-based content with function declarations).
- **FR-005**: The adapter MUST classify HTTP errors using the shared error classifier (429 → rate limit, 401/403 → auth, 5xx → network, timeout → network).
- **FR-006**: The adapter MUST handle Gemini safety filter blocks, surfacing them as errors rather than silently dropping content.

### Key Entities

- **GeminiStreamFn**: The streaming function that connects to the Google Gemini streaming endpoint and produces assistant message events.
- **Function Declaration**: Gemini's representation of tool definitions, distinct from OpenAI-style tool schemas.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Text responses stream incrementally — each delta arrives as a separate event, not buffered until completion.
- **SC-002**: Function calls produce valid, parseable JSON arguments upon completion.
- **SC-003**: Message format conversion preserves all content types (text, tool calls, tool results, images) without data loss.
- **SC-004**: All Gemini error codes and safety blocks map to the correct agent error types consistently.

## Assumptions

- The Gemini API uses Server-Sent Events for streaming.
- Gemini uses a parts-based content model and function declarations, distinct from OpenAI-style messages and tools.
- The shared conversion trait and error classifier from the adapter shared infrastructure (spec 011) are available.
- The adapter does not manage API key storage — credentials are provided by the caller.
- Safety filter blocks are treated as error conditions, not silently swallowed.
