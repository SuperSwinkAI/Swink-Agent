# Feature Specification: Adapter: Mistral

**Feature Branch**: `018-adapter-mistral`
**Created**: 2026-03-20
**Status**: Draft
**Input**: MistralStreamFn for Mistral API via SSE. References: PRD §15.1, HLD Adapters.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Stream Text Responses from Mistral (Priority: P1)

A developer configures the Mistral adapter with an API key and model selection and sends a conversation to the Mistral chat completions endpoint. The adapter streams back text content in real time as Server-Sent Events, delivering each text delta to the agent loop as it arrives. The developer sees assistant responses appear incrementally.

**Why this priority**: Streaming text is the fundamental capability — without it, the adapter has no value.

**Independent Test**: Can be tested by sending a simple prompt to the Mistral endpoint and verifying that text deltas arrive incrementally and the final assembled message is coherent.

**Acceptance Scenarios**:

1. **Given** valid Mistral credentials and a model selection, **When** a conversation is sent, **Then** text content streams back incrementally via SSE.
2. **Given** a streaming response, **When** all deltas have arrived, **Then** the assembled message matches what the provider produced.
3. **Given** a streaming response, **When** the stream completes normally, **Then** a terminal event signals completion.

---

### User Story 2 - Stream Tool Call Responses from Mistral (Priority: P1)

A developer sends a conversation with tool definitions to the Mistral endpoint. The adapter streams back tool call chunks, including the tool name, tool call ID, and incrementally-arriving JSON arguments. The agent loop receives structured tool call events that it can dispatch for execution.

**Why this priority**: Tool calling is essential for agentic workflows — the primary use case of this library.

**Independent Test**: Can be tested by sending a prompt with tool definitions that the model is likely to invoke, and verifying that tool call events arrive with correct names, IDs, and parseable arguments.

**Acceptance Scenarios**:

1. **Given** a conversation with tool definitions, **When** the model decides to call a tool, **Then** the adapter emits tool call start, delta, and end events.
2. **Given** streamed tool call arguments, **When** the tool call ends, **Then** the accumulated arguments form valid JSON.
3. **Given** a response with multiple tool calls, **When** streamed, **Then** each tool call is emitted as a separate indexed block with its own ID.

---

### User Story 3 - Connect to Mistral-Specific Endpoint (Priority: P2)

A developer configures the Mistral adapter with the Mistral-specific base URL and authentication. The adapter targets the Mistral endpoint which follows the OpenAI chat completions protocol but has its own base URL and may have provider-specific behaviors (e.g., different tool calling conventions or response format quirks). The adapter handles any Mistral-specific differences transparently.

**Why this priority**: Mistral endpoint targeting is what distinguishes this adapter, but the streaming protocol is largely standard.

**Independent Test**: Can be tested by verifying that the adapter constructs the correct Mistral URL and includes proper authentication.

**Acceptance Scenarios**:

1. **Given** Mistral credentials, **When** a request is made, **Then** the correct Mistral endpoint URL is used.
2. **Given** Mistral authentication, **When** a request is made, **Then** the API key is included in the correct header.
3. **Given** a Mistral-specific response format quirk, **When** parsed, **Then** it is handled gracefully without errors.

---

### User Story 4 - Handle Errors from Mistral (Priority: P2)

A developer encounters various error conditions when communicating with the Mistral endpoint (invalid key, rate limiting, server errors, network timeouts). The adapter classifies these errors using the shared error classifier so that the agent loop can apply appropriate retry strategies.

**Why this priority**: Correct error handling enables reliable operation, but the adapter can demonstrate value with the happy path alone.

**Independent Test**: Can be tested by simulating error responses (429, 401, 500, network timeout) and verifying each maps to the correct error type.

**Acceptance Scenarios**:

1. **Given** an HTTP 429 response, **When** classified, **Then** it maps to a rate-limit error (retryable).
2. **Given** an HTTP 401 response, **When** classified, **Then** it maps to an authentication error (not retryable).
3. **Given** a network timeout, **When** classified, **Then** it maps to a network error (retryable).
4. **Given** an HTTP 500 response, **When** classified, **Then** it maps to a network error (retryable).

---

### Edge Cases

- What happens when Mistral returns tool calls in a format that differs slightly from OpenAI's (e.g., different field names)?
- How does the adapter handle Mistral-specific rate limiting patterns or headers?
- What happens when the Mistral API returns an error format different from the expected schema?
- How does the adapter handle Mistral model names that are not recognized by the catalog?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The adapter MUST stream text responses from the Mistral chat completions endpoint via SSE, emitting incremental text deltas.
- **FR-002**: The adapter MUST stream tool call responses, emitting tool name, tool call ID, argument deltas, and completion events.
- **FR-003**: The adapter MUST target the Mistral-specific endpoint URL with proper authentication.
- **FR-004**: The adapter MUST convert agent messages to the chat completions message format using the shared conversion trait.
- **FR-005**: The adapter MUST classify HTTP errors using the shared error classifier (429 → rate limit, 401/403 → auth, 5xx → network, timeout → network).
- **FR-006**: The adapter MUST handle Mistral-specific response quirks gracefully without panics or data loss.

### Key Entities

- **MistralStreamFn**: The streaming function that connects to the Mistral chat completions endpoint and produces assistant message events.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Text responses stream incrementally — each delta arrives as a separate event, not buffered until completion.
- **SC-002**: Tool calls produce valid, parseable JSON arguments upon completion.
- **SC-003**: The adapter correctly targets the Mistral endpoint and authenticates successfully.
- **SC-004**: All Mistral error codes map to the correct agent error types consistently.

## Assumptions

- The Mistral API largely follows the OpenAI chat completions protocol (SSE streaming, similar message format) with its own endpoint and authentication.
- The shared conversion trait and error classifier from the adapter shared infrastructure (spec 011) are available.
- The adapter does not manage API key storage — credentials are provided by the caller.
- Mistral-specific behaviors are handled by lenient parsing rather than dedicated code paths.
