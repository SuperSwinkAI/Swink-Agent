# Feature Specification: Adapter Shared Infrastructure

**Feature Branch**: `011-adapter-shared-infra`
**Created**: 2026-03-20
**Status**: Draft
**Input**: Shared infrastructure that all LLM provider adapters depend on: message conversion trait, HTTP error classification, SSE parsing helpers, catalog-driven remote connection construction, prompt caching strategy configuration, proxy streaming mode, and raw provider payload callback. References: PRD §15.1 (Adapters Crate), HLD Adapters Crate (convert, classify, sse, remote_presets).

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

### User Story 5 - Configure Prompt Caching Strategy (Priority: P2) — I6

An adapter developer configures a caching strategy that adapters use to inject cache control markers into LLM requests. The strategy is provider-agnostic at the configuration level — the adapter translates it to provider-specific format (e.g., Anthropic `cache_control` blocks, Google `CachedContent` resources). Adapters that don't support caching treat the strategy as a no-op.

**Why this priority**: Prompt caching can reduce cost by 90% for repeated system prompts and tool definitions. Without a shared abstraction, each adapter would implement caching differently, making it hard for users to switch providers.

**Independent Test**: Can be tested by configuring `CacheStrategy::Auto`, calling an adapter's `apply_cache_strategy()`, and verifying the correct provider-specific markers are injected into the request.

**Acceptance Scenarios**:

1. **Given** `CacheStrategy::None` (default), **When** an adapter builds a request, **Then** no cache markers are injected.
2. **Given** `CacheStrategy::Auto`, **When** the Anthropic adapter builds a request, **Then** `cache_control: { type: "ephemeral" }` is injected on the system prompt and tool definitions.
3. **Given** `CacheStrategy::Auto`, **When** an adapter that doesn't support caching builds a request, **Then** the strategy is silently ignored (graceful degradation).
4. **Given** `CacheStrategy::Google { ttl }`, **When** the Google adapter builds a request, **Then** a `CachedContent` resource is referenced with the specified TTL.
5. **Given** a `CacheStrategy`, **When** it is passed through `StreamOptions`, **Then** each adapter's streaming function receives it and applies it at request construction time.

---

### User Story 6 - Proxy Raw SSE Bytes Without Parsing (Priority: P3) — N3

A gateway developer uses a `ProxyStreamFn` to relay raw SSE bytes from a provider to a consumer without parsing them into `AssistantMessageEvent`. This enables Swink to act as a thin proxy in gateway deployments where the consumer handles its own event parsing.

**Why this priority**: Nice-to-have for gateway deployments. Most users use the full event-parsing pipeline.

**Independent Test**: Can be tested by configuring a `ProxyStreamFn`, sending a request, and verifying the consumer receives raw SSE bytes matching what the provider sent.

**Acceptance Scenarios**:

1. **Given** a `ProxyStreamFn` configured for an Anthropic endpoint, **When** a streaming request is made, **Then** the consumer receives raw `Bytes` chunks from the provider's SSE stream.
2. **Given** a `ProxyStreamFn`, **When** the stream ends, **Then** the consumer receives no synthetic events — just the raw provider data.
3. **Given** a `ProxyStreamFn`, **When** the provider returns an HTTP error, **Then** the error is propagated as-is to the consumer.

---

### User Story 7 - Observe Raw Provider Payloads (Priority: P3) — N4

A developer configures an optional callback in `StreamOptions` that fires with each raw SSE data line before it enters the adapter's event parsing state machine. This is a low-level escape hatch for debugging, logging, or handling provider-specific extensions.

**Why this priority**: Nice-to-have debugging tool. Most users don't need raw payload access.

**Independent Test**: Can be tested by configuring an `on_raw_payload` callback, sending a request, and verifying the callback fires with each raw SSE data line.

**Acceptance Scenarios**:

1. **Given** an `on_raw_payload` callback configured in `StreamOptions`, **When** the adapter receives SSE data lines, **Then** the callback is called with each raw data line string before event parsing.
2. **Given** no `on_raw_payload` callback (default), **When** the adapter receives SSE data lines, **Then** no overhead is added (the callback check is a simple `Option::is_some`).
3. **Given** an `on_raw_payload` callback that panics, **When** it panics during invocation, **Then** the panic is caught and the streaming pipeline continues uninterrupted.
4. **Given** an `on_raw_payload` callback, **When** it is invoked, **Then** it MUST NOT block the streaming pipeline (fire-and-forget semantics — the callback receives a `&str` reference and returns nothing).

---

### Edge Cases

- What happens when an HTTP response has no status code (connection reset) — the status code classifier only handles HTTP status codes. Connection-level errors (resets, timeouts) are mapped to `NetworkError` at the adapter level by reqwest error handling.
- How does SSE parsing handle events with no data field — events without data are valid SSE (comment lines, event-type-only lines) and are parsed normally as their respective line types.
- What happens when a preset references a provider not supported by any installed adapter — the connection factory returns None/error; the caller handles it (e.g., TUI shows available providers).
- What happens when `CacheStrategy::Anthropic` is used with a non-Anthropic adapter — silently ignored. Each adapter only applies strategies it understands.
- What happens when `CacheStrategy::Google { ttl }` is set but the Google adapter can't create a cached content resource — falls back to uncached request with a logged warning.
- What happens when the `on_raw_payload` callback is slow — it executes synchronously on the streaming task. A slow callback delays event delivery. The callback contract requires it to return quickly (fire-and-forget). This is documented, not enforced at runtime.
- What happens when `ProxyStreamFn` is used with a model that requires custom authentication (e.g., Bedrock SigV4) — `ProxyStreamFn` reuses `AdapterBase` for connection management, which handles auth. The proxy doesn't parse events but still authenticates correctly.

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
- **FR-009**: System MUST provide a `CacheStrategy` enum (`None`, `Auto`, `Anthropic`, `Google { ttl }`) that flows from `StreamOptions` to adapters for provider-specific cache marker injection.
- **FR-010**: Adapters that support caching MUST implement `apply_cache_strategy()` to translate the `CacheStrategy` into provider-specific request modifications. Adapters that don't support caching MUST silently ignore the strategy.
- **FR-011**: System MUST provide a `ProxyStreamFn` that relays raw SSE bytes from a provider without parsing them into `AssistantMessageEvent`.
- **FR-012**: System MUST provide an optional `on_raw_payload: Option<OnRawPayload>` field in `StreamOptions` that receives raw SSE data line strings before event parsing.
- **FR-013**: The `on_raw_payload` callback MUST NOT block the streaming pipeline. Panics in the callback MUST be caught and the stream MUST continue.

### Key Entities

- **MessageConverter**: Trait for converting agent messages to provider-specific formats.
- **HttpErrorClassifier**: Maps HTTP status codes and network errors to agent error types.
- **SSE Parser**: Shared helpers for consuming Server-Sent Events streams.
- **RemotePresets**: Catalog-driven factory for constructing configured remote connections.
- **CacheStrategy**: Provider-agnostic caching configuration that adapters translate to provider-specific markers.
- **ProxyStreamFn**: Raw SSE byte relay for gateway deployments — skips event parsing.
- **OnRawPayload**: Optional callback for observing raw SSE data lines before event parsing.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The conversion trait correctly handles all message and content block types.
- **SC-002**: HTTP error classification is consistent: same status code always maps to the same error type.
- **SC-003**: SSE parsing correctly extracts event data from valid streams and reports errors for malformed streams.
- **SC-004**: Catalog presets resolve to correctly configured remote connections with appropriate credentials and endpoints.
- **SC-005**: `CacheStrategy::Auto` correctly injects Anthropic cache markers when used with the Anthropic adapter and is silently ignored by adapters without caching support.
- **SC-006**: `ProxyStreamFn` delivers raw SSE bytes to the consumer without any `AssistantMessageEvent` conversion.
- **SC-007**: `on_raw_payload` fires for each raw SSE data line and a panicking callback does not interrupt the stream.

## Clarifications

### Session 2026-03-20

- Q: How are connection resets (no HTTP status) classified? → A: Adapter-level reqwest error handling maps them to NetworkError. The status classifier only handles HTTP codes.
- Q: How does SSE parsing handle events with no data field? → A: Valid SSE; parsed normally as their respective line types.
- Q: What if a preset references an unsupported provider? → A: Connection factory returns None/error; caller handles it.

## Assumptions

- SSE is the dominant streaming protocol (used by 7 of 9 adapters). NDJSON (Ollama) is handled separately.
- The error classifier is a utility function, not a trait — all adapters use the same classification logic.
- The adapters crate depends only on the core `swink-agent` crate, not on memory, eval, or TUI.
- `CacheStrategy` is defined in the core crate (in `StreamOptions`) so it's provider-agnostic. Adapters translate it to provider-specific formats.
- `CacheStrategy::Auto` is the recommended default for users who want caching — each adapter determines optimal cache points for its provider.
- `ProxyStreamFn` reuses `AdapterBase` for HTTP/auth but returns `Stream<Item = Bytes>` instead of `Stream<Item = AssistantMessageEvent>`.
- `on_raw_payload` is synchronous and runs on the streaming task. The contract requires it to return quickly — enforcement is by documentation, not runtime timeout.
