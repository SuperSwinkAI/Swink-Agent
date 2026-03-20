# Feature Specification: Adapter: Proxy

**Feature Branch**: `020-adapter-proxy`
**Created**: 2026-03-20
**Status**: Draft
**Input**: ProxyStreamFn for HTTP proxy forwarding via SSE. Bearer token authentication. Delta reconstruction (partial_message stripped by proxy, reconstructed client-side). Error classification (connection/auth/rate-limit/malformed). References: PRD §7.4, §15.1, HLD Adapters.

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

### User Story 2 - Reconstruct Deltas Stripped by the Proxy (Priority: P1)

A developer streams responses through a proxy that strips `partial_message` fields from SSE events (a common proxy optimization to reduce bandwidth). The adapter reconstructs the stripped deltas client-side by tracking the cumulative message state and computing the difference between consecutive events. The developer receives the same incremental deltas they would get from a direct connection, despite the proxy's optimization.

**Why this priority**: Delta reconstruction is the core differentiator of the Proxy adapter — without it, tool call arguments and text deltas would be lost when the proxy strips partial state.

**Independent Test**: Can be tested by feeding a sequence of SSE events with stripped partial_message fields and verifying that the adapter reconstructs the correct deltas.

**Acceptance Scenarios**:

1. **Given** SSE events with partial_message stripped, **When** processed sequentially, **Then** the adapter reconstructs the correct text deltas by diffing consecutive states.
2. **Given** SSE events with tool call arguments stripped, **When** processed, **Then** the adapter reconstructs the correct argument deltas.
3. **Given** a mix of stripped and non-stripped events, **When** processed, **Then** the adapter handles both correctly without double-counting content.

---

### User Story 3 - Authenticate with Bearer Tokens (Priority: P2)

A developer provides a bearer token for proxy authentication. The adapter includes the token in every request to the proxy. If the token is invalid or expired, the error is clearly surfaced as an authentication failure.

**Why this priority**: Authentication is required for the proxy to accept requests, but it is infrastructure rather than the core streaming experience.

**Independent Test**: Can be tested by sending a request with a valid token and verifying it succeeds, then with an invalid token and verifying the authentication error.

**Acceptance Scenarios**:

1. **Given** a valid bearer token, **When** a request is sent to the proxy, **Then** the token is included in the authorization header and the request succeeds.
2. **Given** an invalid or expired bearer token, **When** a request is sent, **Then** the adapter surfaces an authentication error (not retryable).

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

- What happens when the proxy strips partial_message from some events but not others within the same stream?
- How does the adapter handle a proxy that silently drops events (gaps in the sequence)?
- What happens when the proxy returns a non-SSE response (e.g., an HTML error page)?
- How does delta reconstruction handle a reset in the cumulative state (e.g., the proxy restarts mid-stream)?
- What happens when the proxy adds latency that causes the connection to appear timed out?
- How does the adapter distinguish between a proxy authentication failure and an upstream provider authentication failure?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The adapter MUST stream text responses through the proxy via SSE, emitting incremental text deltas.
- **FR-002**: The adapter MUST reconstruct deltas when partial_message fields are stripped by the proxy, by computing differences between consecutive cumulative states.
- **FR-003**: The adapter MUST authenticate with the proxy using bearer token authorization.
- **FR-004**: The adapter MUST classify errors into four categories: connection errors (retryable), authentication errors (not retryable), rate-limit errors (retryable), and malformed response errors (with diagnostic context).
- **FR-005**: The adapter MUST handle tool call delta reconstruction, not just text deltas.
- **FR-006**: The adapter MUST handle mixed streams where some events have partial_message and others do not.
- **FR-007**: The adapter MUST surface malformed proxy responses as parse errors with enough context for debugging.

### Key Entities

- **ProxyStreamFn**: The streaming function that connects through an HTTP proxy and produces assistant message events, reconstructing stripped deltas.
- **Delta Reconstructor**: The component that tracks cumulative message state and computes deltas when partial_message is stripped.
- **Bearer Token**: Authentication credential for the proxy endpoint.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Text responses stream incrementally through the proxy — each delta arrives as a separate event.
- **SC-002**: Delta reconstruction produces identical event sequences whether or not the proxy strips partial_message fields.
- **SC-003**: All four error categories (connection, auth, rate-limit, malformed) are correctly classified.
- **SC-004**: Bearer token authentication is included in every request to the proxy.

## Assumptions

- The proxy forwards requests to an upstream LLM provider and relays the SSE stream back.
- The proxy may strip partial_message fields as a bandwidth optimization — the adapter must handle this.
- Bearer token is the authentication mechanism for the proxy (distinct from any upstream provider authentication, which the proxy handles).
- The shared error classifier from the adapter shared infrastructure (spec 011) is available, extended with proxy-specific error categories.
- The proxy does not modify the semantic content of the stream — only strips optional fields.
