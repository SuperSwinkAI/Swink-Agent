# Research: Adapter: Proxy

**Feature**: 020-adapter-proxy | **Date**: 2026-03-20

## Decision 1: SSE Protocol via eventsource-stream

**Question**: Should the proxy adapter use the shared `SseStreamParser` from spec 011 or the `eventsource-stream` crate?

**Decision**: Use `eventsource-stream`. The proxy adapter calls `.bytes_stream().eventsource()` on the reqwest response and processes fully parsed SSE events.

**Rationale**: The shared `SseStreamParser` from spec 011 exists for adapters that need low-level control over SSE event-type labels (e.g., Anthropic's state machine driven by `event:` labels). The proxy protocol uses a simpler model: every SSE event carries a self-describing JSON payload with a `type` field. `eventsource-stream` handles line buffering, reconnection hints, and UTF-8 decoding — functionality that would need to be reimplemented otherwise. The crate is well-maintained and already a workspace dependency.

**Alternatives rejected**:
- *Shared `SseStreamParser`*: Designed for adapters that dispatch on `event:` labels. The proxy dispatches on the JSON `type` field inside `data:`, making `SseStreamParser`'s `SseLine::Event` variant unused overhead.
- *Raw byte-stream parsing*: Reimplements what `eventsource-stream` already provides with no benefit.

## Decision 2: Typed Event Enum (not partial_message Diffing)

**Question**: Should the adapter reconstruct deltas by diffing consecutive `partial_message` snapshots, as the spec's user story 2 suggests?

**Decision**: No delta reconstruction. The proxy protocol uses typed SSE events (`TextDelta`, `ToolCallDelta`, `ThinkingDelta`, etc.) where each event carries the incremental delta directly. The `SseEventData` enum maps 1:1 to `AssistantMessageEvent` variants.

**Rationale**: The spec's user story 2 (delta reconstruction) was written before the protocol was finalized. The edge cases section (lines 76-79) clarifies that "the adapter uses typed SSE events, not partial_message fields" and "the adapter uses discrete delta events, not cumulative state diffing." This means delta reconstruction is not applicable — each event is already an incremental delta. The `SseEventData` enum with `#[serde(tag = "type")]` provides type-safe deserialization directly into the correct variant.

**Alternatives rejected**:
- *Cumulative state diffing*: Would require tracking full message state and computing string diffs — complex, error-prone, and unnecessary given the typed protocol.
- *Hybrid approach*: Supporting both typed events and partial_message diffing adds complexity for a scenario that the protocol design explicitly avoids.

## Decision 3: Bearer Token Authentication

**Question**: How should authentication be handled?

**Decision**: Bearer token passed in the `Authorization` header via `reqwest`'s `.bearer_auth()`. The token is stored in `ProxyStreamFn` and can be overridden per-request via `StreamOptions.api_key`.

**Rationale**: Bearer token is the standard HTTP authentication mechanism for API proxies. Storing it in the struct allows reuse across requests. The per-request override via `StreamOptions.api_key` enables scenarios where different conversations use different tokens (e.g., multi-tenant proxy). The `Debug` impl redacts the token to prevent accidental logging.

**Alternatives rejected**:
- *Custom header*: Non-standard; would require proxy-specific documentation for every consumer.
- *Query parameter*: Tokens appear in server logs and URL bars — security risk.
- *mTLS*: Heavyweight; not justified for a library-level adapter.

## Decision 4: Error Classification via Shared Classifier

**Question**: Should the proxy adapter have its own error classification or reuse the shared infrastructure?

**Decision**: Reuse `classify_http_status` from `crate::classify` (spec 011). The proxy adapter calls it in `classify_response_status` and maps `HttpErrorKind` variants to the corresponding `AssistantMessageEvent::error_*` constructors.

**Rationale**: The proxy uses standard HTTP status codes (401 = auth, 429 = throttled, 5xx = network). The shared classifier already handles these cases. No provider-specific overrides are needed — the proxy protocol doesn't use non-standard status codes. Connection errors from reqwest are caught at the `send()` call and mapped to `error_network`.

**Alternatives rejected**:
- *Custom classifier*: Would duplicate the shared logic with no additional value.
- *`classify_with_overrides`*: No proxy-specific status codes need overriding.

## Decision 5: No Delta Reconstruction (Events are Discrete)

**Question**: Does the adapter need a delta reconstruction component?

**Decision**: No. The `Delta Reconstructor` entity mentioned in the spec's key entities is not needed.

**Rationale**: Per the spec's own clarifications (session 2026-03-20): "Not applicable; adapter uses typed SSE events, not partial_message" and "Not applicable; discrete delta events, not cumulative diffing." The `SseEventData` enum's `TextDelta`, `ToolCallDelta`, and `ThinkingDelta` variants each carry the incremental `delta: String` field directly. The `convert_sse_event` function is a straightforward 1:1 mapping with no state.

**Alternatives rejected**:
- *Implementing delta reconstruction anyway*: Would add dead code and maintenance burden for a feature the protocol design explicitly avoids.
