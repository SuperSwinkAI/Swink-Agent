# Feature Specification: Adapter: OpenAI

**Feature Branch**: `013-adapter-openai`
**Created**: 2026-03-20
**Status**: Draft
**Input**: OpenAiStreamFn for /v1/chat/completions via SSE. Multi-provider compatible (vLLM, LM Studio, Groq, Together, etc.). OpenAI message format conversion. References: PRD §15.1, HLD Adapters.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Stream Text Responses from OpenAI-Compatible Providers (Priority: P1)

A developer configures the OpenAI adapter with an API key, base URL, and model selection and sends a conversation to the chat completions endpoint. The adapter streams back text content in real time as Server-Sent Events, delivering each text delta to the agent loop as it arrives. The developer sees assistant responses appear incrementally.

**Why this priority**: Streaming text is the fundamental capability — without it, the adapter has no value.

**Independent Test**: Can be tested by sending a simple prompt to the OpenAI endpoint and verifying that text deltas arrive incrementally and the final assembled message is coherent.

**Acceptance Scenarios**:

1. **Given** valid credentials and a model selection, **When** a conversation is sent, **Then** text content streams back incrementally via SSE.
2. **Given** a streaming response, **When** all deltas have arrived, **Then** the assembled message matches what the provider produced.
3. **Given** a streaming response, **When** the stream ends with a `[DONE]` sentinel, **Then** a terminal event signals completion.

---

### User Story 2 - Stream Tool Call Responses from OpenAI-Compatible Providers (Priority: P1)

A developer sends a conversation with tool definitions to the chat completions endpoint. The adapter streams back tool call chunks, including the tool name, tool call ID, and incrementally-arriving JSON arguments. The agent loop receives structured tool call events that it can dispatch for execution.

**Why this priority**: Tool calling is essential for agentic workflows — the primary use case of this library.

**Independent Test**: Can be tested by sending a prompt with tool definitions that the model is likely to invoke, and verifying that tool call events arrive with correct names, IDs, and parseable arguments.

**Acceptance Scenarios**:

1. **Given** a conversation with tool definitions, **When** the model decides to call a tool, **Then** the adapter emits tool call start, delta, and end events.
2. **Given** streamed tool call arguments, **When** the tool call ends, **Then** the accumulated arguments form valid JSON.
3. **Given** a response with multiple parallel tool calls, **When** streamed, **Then** each tool call is emitted as a separate indexed block with its own ID.

---

### User Story 3 - Connect to Alternative OpenAI-Compatible Providers (Priority: P2)

A developer points the OpenAI adapter at a non-OpenAI provider (such as a local inference server, Groq, Together, or any other service that implements the OpenAI chat completions protocol) by changing the base URL. The adapter works without modification because it targets the protocol, not a specific provider. Provider-specific quirks (e.g., missing fields, different finish reasons) are handled gracefully.

**Why this priority**: Multi-provider compatibility dramatically increases the adapter's reach, but the adapter is useful with just the OpenAI endpoint.

**Independent Test**: Can be tested by configuring the adapter with different base URLs and verifying that streaming works correctly across providers.

**Acceptance Scenarios**:

1. **Given** a custom base URL pointing to an alternative provider, **When** a conversation is sent, **Then** text streams correctly.
2. **Given** a provider that omits optional fields in SSE chunks, **When** streamed, **Then** the adapter handles missing fields gracefully without errors.
3. **Given** a provider with a different finish reason vocabulary, **When** the stream completes, **Then** the adapter still signals completion correctly.

---

### User Story 4 - Handle Errors from OpenAI-Compatible Providers (Priority: P2)

A developer encounters various error conditions (invalid key, rate limiting, server errors, network timeouts). The adapter classifies these errors using the shared error classifier so that the agent loop can apply appropriate retry strategies.

**Why this priority**: Correct error handling enables reliable operation, but the adapter can demonstrate value with the happy path alone.

**Independent Test**: Can be tested by simulating error responses (429, 401, 500, network timeout) and verifying each maps to the correct error type.

**Acceptance Scenarios**:

1. **Given** an HTTP 429 response, **When** classified, **Then** it maps to a rate-limit error (retryable) with retry-after timing if provided.
2. **Given** an HTTP 401 response, **When** classified, **Then** it maps to an authentication error (not retryable).
3. **Given** a network timeout, **When** classified, **Then** it maps to a network error (retryable).
4. **Given** an HTTP 500 or 502 response, **When** classified, **Then** it maps to a network error (retryable).

---

### Edge Cases

- What happens when an alternative provider returns chunks in a different order — adapter iterates choices and processes whatever fields are present; order-tolerant.
- How does the adapter handle a `[DONE]` sentinel that arrives mid-content — stream end without `[DONE]` is handled gracefully; open blocks are finalized if a stop_reason was set.
- What happens when a provider returns an empty choices array — empty iteration does nothing; no crash or error.
- How are tool calls handled when the provider omits the tool call ID — a UUID is auto-generated (`tc_{uuid}`).
- What happens when the provider returns an unrecognized finish_reason — defaults to `StopReason::Stop` via wildcard match.
- **Tool-call start timing**: The adapter buffers tool call data until both a `content_index` and a tool name are known before emitting `ToolCallStart`. A `ToolCallStart` event is never emitted with an empty or unknown tool name.
- **`content_filter` finish reason is terminal**: When the provider returns `finish_reason: "content_filter"`, the adapter treats this as a terminal error condition and emits `AssistantMessageEvent::error_content_filtered()`. It does not fall through to the normal `StopReason::Stop` mapping.
- **Shared transport layer**: This adapter is implemented atop a shared `openai_compat` module that handles SSE parsing, tool call accumulation, and finish-reason classification. The adapter provides provider-specific configuration (endpoint, auth, model mapping) to this shared shell.
- **Base URL trailing slash normalization**: Trailing slashes on the configured base URL are stripped before constructing request paths, preventing double-slash errors (e.g., `https://api.openai.com/v1/` → `https://api.openai.com/v1`).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The adapter MUST stream text responses from the chat completions endpoint via SSE, emitting incremental text deltas.
- **FR-002**: The adapter MUST stream tool call responses, emitting tool name, tool call ID, argument deltas, and completion events.
- **FR-003**: The adapter MUST support configurable base URLs to connect to any OpenAI-compatible provider.
- **FR-004**: The adapter MUST convert agent messages to the OpenAI chat completions message format using the shared conversion trait.
- **FR-005**: The adapter MUST classify HTTP errors using the shared error classifier (429 → rate limit, 401/403 → auth, 5xx → network, timeout → network).
- **FR-006**: The adapter MUST handle the `[DONE]` sentinel as the stream termination signal.
- **FR-007**: The adapter MUST handle missing or unexpected fields in SSE chunks gracefully (no panics, no silent data loss).

### Key Entities

- **OpenAiStreamFn**: The streaming function that connects to any OpenAI-compatible chat completions endpoint and produces assistant message events.
- **Base URL**: Configurable endpoint URL that enables multi-provider compatibility.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Text responses stream incrementally — each delta arrives as a separate event, not buffered until completion.
- **SC-002**: Tool calls produce valid, parseable JSON arguments upon completion.
- **SC-003**: The adapter works with at least one alternative provider (non-OpenAI) without code changes — only configuration differs.
- **SC-004**: All error codes map to the correct agent error types consistently.

## Clarifications

### Session 2026-03-20

- Q: Chunks in different order from alternative providers? → A: Order-tolerant; processes whatever fields present.
- Q: `[DONE]` mid-content or missing? → A: Gracefully finalizes blocks if stop_reason was set.
- Q: Empty choices array? → A: No-op; no crash.
- Q: Missing tool call ID? → A: Auto-generates UUID (`tc_{uuid}`).
- Q: Unrecognized finish_reason? → A: Defaults to `StopReason::Stop`.

## Assumptions

- The OpenAI chat completions SSE protocol is the de facto standard — most alternative providers implement it.
- The shared conversion trait and error classifier from the adapter shared infrastructure (spec 011) are available.
- The adapter does not manage API key storage — credentials are provided by the caller.
- Provider-specific quirks are handled by being lenient in parsing, not by provider-specific code paths.
