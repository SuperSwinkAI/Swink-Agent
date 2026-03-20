# Feature Specification: Adapter Shared Infrastructure

**Feature Branch**: `011-adapter-shared-infra`
**Created**: 2026-03-20
**Status**: Draft
**Input**: Shared infrastructure that all LLM provider adapters depend on: message conversion trait, HTTP error classification, SSE parsing helpers, and catalog-driven remote connection construction. References: PRD §15.1 (Adapters Crate), HLD Adapters Crate (convert, classify, sse, remote_presets).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Convert Messages to Provider Format (Priority: P1)

An adapter developer converts agent messages to a specific provider's expected format using a shared conversion trait. The trait defines the contract for converting messages, content blocks, tool calls, and tool results. Each adapter implements this trait for its provider, ensuring consistent conversion logic across all adapters.

**Why this priority**: Every adapter must convert messages. A shared trait prevents duplication and ensures consistency.

**Independent Test**: Can be tested by implementing the conversion trait for a mock provider format and verifying all message types convert correctly.

**Acceptance Scenarios**:

1. **Given** the conversion trait, **When** an adapter implements it, **Then** user messages, assistant messages, and tool results are converted to the provider format.
2. **Given** content blocks, **When** they are converted, **Then** text, thinking, tool call, and image blocks are each handled appropriately.
3. **Given** the conversion trait, **When** a new adapter is created, **Then** it implements the same trait as all other adapters.

---

### User Story 2 - Classify HTTP Errors Consistently (Priority: P1)

An adapter developer uses a shared error classifier to map HTTP status codes and network errors to the correct agent error types. The classifier ensures that 429 always maps to rate limiting, 401/403 always maps to authentication failure, network timeouts always map to network error, etc. — consistently across all adapters.

**Why this priority**: Consistent error classification is essential for the retry strategy to work correctly across all providers.

**Independent Test**: Can be tested by passing various HTTP status codes to the classifier and verifying the correct agent error types are returned.

**Acceptance Scenarios**:

1. **Given** an HTTP 429 response, **When** classified, **Then** it maps to a rate-limit error (retryable).
2. **Given** an HTTP 401 or 403 response, **When** classified, **Then** it maps to a stream error (not retryable).
3. **Given** a connection timeout, **When** classified, **Then** it maps to a network error (retryable).
4. **Given** an HTTP 500 or 502 response, **When** classified, **Then** it maps to a network error (retryable).

---

### User Story 3 - Parse SSE Streams (Priority: P1)

An adapter developer uses shared SSE parsing helpers to consume Server-Sent Events from a provider's streaming endpoint. The helpers handle the SSE protocol (event names, data fields, line parsing) and produce parsed event payloads that the adapter can then map to assistant message events.

**Why this priority**: Most providers (7 of 9) use SSE for streaming. Shared parsing prevents duplicating protocol handling.

**Independent Test**: Can be tested by feeding raw SSE text to the parser and verifying correctly parsed events are produced.

**Acceptance Scenarios**:

1. **Given** raw SSE text with data events, **When** parsed, **Then** each event's data payload is correctly extracted.
2. **Given** an SSE stream that ends with a terminal event, **When** parsed, **Then** the terminal condition is detected.
3. **Given** malformed SSE text, **When** parsed, **Then** a parse error is produced rather than a silent failure.

---

### User Story 4 - Construct Connections from Catalog Presets (Priority: P2)

A developer selects a model from the catalog and the system constructs a fully configured remote connection, including the appropriate streaming function, from the catalog preset. This eliminates manual wiring — the catalog drives provider selection.

**Why this priority**: Catalog-driven connections reduce boilerplate and ensure presets are usable end-to-end.

**Independent Test**: Can be tested by selecting a catalog preset and verifying a correctly configured connection is produced.

**Acceptance Scenarios**:

1. **Given** a catalog preset for a provider, **When** a remote connection is constructed, **Then** the correct streaming function type is used.
2. **Given** a preset with credentials set via environment variables, **When** the connection is constructed, **Then** it carries the correct credentials.

---

### Edge Cases

- What happens when an HTTP response has no status code (connection reset) — how is it classified?
- How does SSE parsing handle events with no data field?
- What happens when a preset references a provider not supported by any installed adapter?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST provide a message conversion trait that defines the contract for converting agent messages to provider-specific formats.
- **FR-002**: The conversion trait MUST handle all content block types: text, thinking, tool call, and image.
- **FR-003**: System MUST provide an HTTP error classifier that maps status codes and network errors to agent error types consistently.
- **FR-004**: The classifier MUST map: 429 → rate limit (retryable), 401/403 → stream error (not retryable), connection/timeout → network error (retryable), 5xx → network error (retryable).
- **FR-005**: System MUST provide shared SSE parsing helpers for consuming Server-Sent Events streams.
- **FR-006**: SSE parsing MUST handle the standard SSE protocol (event names, data fields, multi-line data, terminal events).
- **FR-007**: System MUST provide catalog-driven remote connection construction that resolves presets to configured streaming functions.
- **FR-008**: The adapters crate MUST re-export all public adapter types and the shared infrastructure from its root module.

### Key Entities

- **MessageConverter**: Trait for converting agent messages to provider-specific formats.
- **HttpErrorClassifier**: Maps HTTP status codes and network errors to agent error types.
- **SSE Parser**: Shared helpers for consuming Server-Sent Events streams.
- **RemotePresets**: Catalog-driven factory for constructing configured remote connections.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The conversion trait correctly handles all message and content block types.
- **SC-002**: HTTP error classification is consistent: same status code always maps to the same error type.
- **SC-003**: SSE parsing correctly extracts event data from valid streams and reports errors for malformed streams.
- **SC-004**: Catalog presets resolve to correctly configured remote connections with appropriate credentials and endpoints.

## Assumptions

- SSE is the dominant streaming protocol (used by 7 of 9 adapters). NDJSON (Ollama) is handled separately.
- The error classifier is a utility function, not a trait — all adapters use the same classification logic.
- The adapters crate depends only on the core `swink-agent` crate, not on memory, eval, or TUI.
