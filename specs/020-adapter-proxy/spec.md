# Feature Specification: Adapter: Proxy

**Feature Branch**: `020-adapter-proxy`
**Created**: 2026-03-20
**Status**: Draft
**Input**: ProxyStreamFn for HTTP proxy forwarding via SSE. Bearer token authentication. Typed SSE event handling (text, thinking, tool call deltas mapped 1:1). Error classification (connection/auth/rate-limit/malformed). References: PRD §7.4, §15.1, HLD Adapters.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Stream Text Responses Through a Proxy (Priority: P1)

A developer configures the Proxy adapter with a proxy URL and bearer token and sends a conversation through the proxy to a remote LLM provider. The proxy forwards the request and relays the SSE stream back. The adapter delivers each text delta to the agent loop as it arrives. The developer sees assistant responses appear incrementally, with the proxy being transparent for text content.

**Why this priority**: Streaming text through the proxy is the fundamental capability — without it, the adapter has no value.

**Independent Test**: Can be tested by sending a simple prompt through a proxy endpoint and verifying that text deltas arrive incrementally and the final assembled message is coherent.

**Acceptance Scenarios**:

1. **Given** a valid proxy URL and bearer token, **When** a conversation is sent, **Then** text content streams back incrementally via SSE through the proxy.
2. **Given** a streaming response, **When** all deltas have arrived, **Then** the assembled message matches what the upstream provider produced.
3. **Given** a streaming response, **When** the stream completes normally, **Then** a terminal event signals completion.

---

### User Story 2 - Handle All SSE Event Types (Priority: P1)

A developer streams responses through the proxy that include not just text deltas but also thinking deltas and tool call deltas. The proxy protocol uses discrete typed SSE events (`TextDelta`, `ThinkingDelta`, `ToolCallDelta`, etc.) where each event carries the incremental delta directly. The adapter maps each typed event 1:1 to the corresponding `AssistantMessageEvent` variant. The developer receives all event types with correct content indices and fields.

**Why this priority**: Tool calls and thinking blocks are essential for agent functionality — without handling all event types, the adapter would only work for simple text responses.

**Independent Test**: Can be tested by feeding SSE events for each variant (thinking, tool call) and verifying correct `AssistantMessageEvent` mapping.

**Acceptance Scenarios**:

1. **Given** SSE events with `thinking_start`, `thinking_delta`, `thinking_end` types, **When** processed, **Then** the adapter maps them to the corresponding `AssistantMessageEvent` variants with correct content indices.
2. **Given** SSE events with `tool_call_start`, `tool_call_delta`, `tool_call_end` types, **When** processed, **Then** the adapter maps them correctly including tool call `id` and `name`.
3. **Given** a stream mixing text, thinking, and tool call events, **When** processed, **Then** all event types are handled correctly without interference.

---

### User Story 3 - Authenticate with Bearer Tokens (Priority: P2)

A developer provides a bearer token for proxy authentication. The adapter includes the token in every request to the proxy. If the token is invalid or expired, the error is clearly surfaced as an authentication failure.

**Why this priority**: Authentication is required for the proxy to accept requests, but it is infrastructure rather than the core streaming experience.

**Independent Test**: Can be tested by sending a request with a valid token and verifying it succeeds, then with an invalid token and verifying the authentication error.

**Acceptance Scenarios**:

1. **Given** a valid bearer token, **When** a request is sent to the proxy, **Then** the token is included in the authorization header and the request succeeds.
2. **Given** an invalid or expired bearer token, **When** a request is sent, **Then** the adapter surfaces an authentication error (not retryable).
3. **Given** `StreamOptions.api_key` is `Some`, **When** a request is sent, **Then** the override token is used instead of the stored bearer token.

---

### User Story 4 - Classify Proxy-Specific Errors (Priority: P2)

A developer encounters various error conditions when communicating through the proxy: connection failures to the proxy itself, authentication failures, rate limiting by the proxy or upstream provider, and malformed responses from a misbehaving proxy. The adapter classifies each error type so the agent loop can apply appropriate retry strategies. Proxy-specific errors (connection to proxy failed, malformed proxy response) are distinguished from upstream provider errors.

**Why this priority**: Correct error classification enables the retry strategy to make good decisions, but the adapter can demonstrate value with the happy path alone.

**Independent Test**: Can be tested by simulating various error conditions and verifying each maps to the correct error type.

**Acceptance Scenarios**:

1. **Given** a connection failure to the proxy, **When** classified, **Then** it maps to a network error (retryable).
2. **Given** an HTTP 401 from the proxy (bad bearer token), **When** classified, **Then** it maps to an authentication error (not retryable).
3. **Given** an HTTP 429 from the proxy or upstream, **When** classified, **Then** it maps to a rate-limit error (retryable).
4. **Given** a malformed SSE response from the proxy, **When** classified, **Then** it maps to a parse error with diagnostic context.

---

### Edge Cases

- What happens when the proxy strips partial_message inconsistently — the proxy adapter uses typed SSE events (TextDelta, ToolCallDelta, etc.) not partial_message fields. Stripping is not applicable to this protocol.
- How does the adapter handle silently dropped events — events are processed as-they-come; gaps manifest as missing content but are not detected at the protocol level.
- What happens when the proxy returns a non-SSE response — eventsource parser fails; adapter emits `error_network("SSE stream error: ...")`.
- How does delta reconstruction handle state resets — not applicable; the adapter uses discrete delta events, not cumulative state diffing.
- What happens when proxy latency causes timeout — caught as network error by reqwest at the connection level.
- How does the adapter distinguish proxy vs upstream auth failure — both 401s map to the same `error_auth`; not distinguished.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The adapter MUST stream text responses through the proxy via SSE, emitting incremental text deltas.
- **FR-002**: The adapter MUST handle all typed SSE event variants (`text_start`, `text_delta`, `text_end`, `thinking_start`, `thinking_delta`, `thinking_end`, `tool_call_start`, `tool_call_delta`, `tool_call_end`, `done`, `error`) by mapping each 1:1 to the corresponding `AssistantMessageEvent`.
- **FR-003**: The adapter MUST authenticate with the proxy using bearer token authorization.
- **FR-004**: The adapter MUST classify errors into four categories: connection errors (retryable), authentication errors (not retryable), rate-limit errors (retryable), and malformed response errors (with diagnostic context).
- **FR-005**: The adapter MUST handle tool call events (`tool_call_start` with `id` and `name`, `tool_call_delta`, `tool_call_end`) mapping each to the corresponding `AssistantMessageEvent` variant.
- **FR-006**: The adapter MUST handle mixed streams containing text, thinking, and tool call events in any interleaved order.
- **FR-007**: The adapter MUST surface malformed proxy responses as parse errors with enough context for debugging.

### Key Entities

- **ProxyStreamFn**: The streaming function that connects through an HTTP proxy and produces assistant message events by mapping typed SSE events 1:1 to `AssistantMessageEvent` variants.
- **SseEventData**: The typed enum representing all SSE event variants (`Start`, `TextDelta`, `ToolCallStart`, `Done`, `Error`, etc.) deserialized from the proxy's JSON payloads.
- **Bearer Token**: Authentication credential for the proxy endpoint.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Text responses stream incrementally through the proxy — each delta arrives as a separate event.
- **SC-002**: All typed SSE event variants (text, thinking, tool call, done, error) are correctly mapped to their corresponding `AssistantMessageEvent` variants.
- **SC-003**: All four error categories (connection, auth, rate-limit, malformed) are correctly classified.
- **SC-004**: Bearer token authentication is included in every request to the proxy.

## Clarifications

### Session 2026-03-20

- Q: Inconsistent partial_message stripping? → A: Not applicable; adapter uses typed SSE events, not partial_message.
- Q: Silently dropped events? → A: Processed as-they-come; gaps not detected at protocol level.
- Q: Non-SSE response? → A: Eventsource parser fails → `error_network`.
- Q: Delta reconstruction state reset? → A: Not applicable; discrete delta events, not cumulative diffing.
- Q: Proxy timeout? → A: Caught as network error by reqwest.
- Q: Proxy vs upstream auth? → A: Both 401 → same `error_auth`; not distinguished.

## Assumptions

- The proxy forwards requests to an upstream LLM provider and relays the SSE stream back.
- The proxy uses typed SSE events (discrete deltas) rather than partial_message fields — no delta reconstruction is needed.
- Bearer token is the authentication mechanism for the proxy (distinct from any upstream provider authentication, which the proxy handles).
- The shared error classifier from the adapter shared infrastructure (spec 011) is available, extended with proxy-specific error categories.
- The proxy does not modify the semantic content of the stream — only strips optional fields.
