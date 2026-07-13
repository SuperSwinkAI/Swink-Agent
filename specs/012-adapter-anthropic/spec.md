# Feature Specification: Adapter: Anthropic

**Feature Branch**: `012-adapter-anthropic`
**Created**: 2026-03-20
**Status**: Draft
**Input**: AnthropicStreamFn for /v1/messages via SSE. Thinking block support with budget control. Anthropic-specific message conversion. References: PRD §15.1, HLD Adapters.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Stream Text Responses from Anthropic (Priority: P1)

A developer configures the Anthropic adapter with an API key and model selection and sends a conversation to the Anthropic messages endpoint. The adapter streams back text content in real time as Server-Sent Events, delivering each text delta to the agent loop as it arrives. The developer sees assistant responses appear incrementally rather than waiting for the full response.

**Why this priority**: Streaming text is the fundamental capability — without it, the adapter has no value.

**Independent Test**: Can be tested by sending a simple prompt to the Anthropic endpoint and verifying that text deltas arrive incrementally and the final assembled message is coherent.

**Acceptance Scenarios**:

1. **Given** valid Anthropic credentials and a model selection, **When** a conversation is sent, **Then** text content streams back incrementally via SSE.
2. **Given** a streaming response, **When** all deltas have arrived, **Then** the assembled message matches what the provider produced.
3. **Given** a streaming response, **When** the stream completes normally, **Then** a terminal event signals completion.

---

### User Story 2 - Stream Tool Call Responses from Anthropic (Priority: P1)

A developer sends a conversation that includes tool definitions to the Anthropic endpoint. The adapter streams back tool call blocks, including the tool name and incrementally-arriving JSON arguments. The agent loop receives structured tool call events that it can dispatch for execution.

**Why this priority**: Tool calling is essential for agentic workflows — the primary use case of this library.

**Independent Test**: Can be tested by sending a prompt with tool definitions that the model is likely to invoke, and verifying that tool call events arrive with correct names and parseable arguments.

**Acceptance Scenarios**:

1. **Given** a conversation with tool definitions, **When** the model decides to call a tool, **Then** the adapter emits tool call start, delta, and end events.
2. **Given** streamed tool call arguments, **When** the tool call ends, **Then** the accumulated arguments form valid JSON.
3. **Given** a response with multiple tool calls, **When** streamed, **Then** each tool call is emitted as a separate indexed block.

---

### User Story 3 - Use Thinking Blocks with Budget Control (Priority: P2)

A developer enables extended thinking for an Anthropic model and sets a thinking budget (maximum tokens the model may use for internal reasoning). The adapter includes thinking configuration in the request and streams back thinking blocks separately from text content. The developer can observe the model's reasoning process and control how much token budget is allocated to thinking versus response.

**Why this priority**: Thinking blocks are an Anthropic-differentiating feature that improves response quality for complex tasks, but the adapter is useful without them.

**Independent Test**: Can be tested by enabling thinking with a budget, sending a complex prompt, and verifying that thinking content arrives as distinct blocks with the budget respected.

**Acceptance Scenarios**:

1. **Given** thinking enabled with a budget, **When** a conversation is sent, **Then** the request includes thinking configuration with the specified budget.
2. **Given** a response with thinking content, **When** streamed, **Then** thinking blocks are emitted as distinct events separate from text content.
3. **Given** thinking is not enabled, **When** a conversation is sent, **Then** no thinking configuration is included and the adapter behaves normally.

---

### User Story 4 - Handle Errors from Anthropic (Priority: P2)

A developer encounters various error conditions when communicating with the Anthropic endpoint (invalid key, rate limiting, server errors, network timeouts). The adapter classifies these errors using the shared error classifier so that the agent loop can apply appropriate retry strategies. Rate-limit errors include retry-after timing when the provider supplies it.

**Why this priority**: Correct error handling enables reliable operation, but the adapter can demonstrate value with the happy path alone.

**Independent Test**: Can be tested by simulating error responses (429, 401, 500, network timeout) and verifying each maps to the correct error type.

**Acceptance Scenarios**:

1. **Given** an HTTP 429 response from Anthropic, **When** classified, **Then** it maps to a rate-limit error (retryable) with retry-after timing if provided.
2. **Given** an HTTP 401 response, **When** classified, **Then** it maps to an authentication error (not retryable).
3. **Given** a network timeout, **When** classified, **Then** it maps to a network error (retryable).
4. **Given** an HTTP 529 (overloaded) response, **When** classified, **Then** it maps to a retryable error.

---

### Edge Cases

- What happens when the Anthropic API returns an `overloaded_error` (529) — classified as `error_network` (retryable).
- How does the adapter handle a thinking block that exceeds the specified budget — budget is set in the request; the provider enforces it. Adapter passes through whatever content arrives.
- What happens when the stream is interrupted mid-thinking-block — same as any interrupted stream; accumulation returns "no terminal event found" error.
- How are empty tool call arguments handled — empty partial JSON string becomes `{}` (per core stream accumulation rules).
- What happens when the API returns a content block type the adapter does not recognize — silently skipped (`_ => {}` match arm).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The adapter MUST stream text responses from the Anthropic messages endpoint via SSE, emitting incremental text deltas.
- **FR-002**: The adapter MUST stream tool call responses, emitting tool name, argument deltas, and completion events.
- **FR-003**: The adapter MUST support extended thinking with a configurable token budget.
- **FR-004**: The adapter MUST convert agent messages to the Anthropic message format using the shared conversion trait.
- **FR-005**: The adapter MUST classify HTTP errors using the shared error classifier (429 → rate limit, 401/403 → auth, 5xx → network, timeout → network).
- **FR-006**: The adapter MUST propagate retry-after timing from rate-limit responses when available.
- **FR-007**: The adapter MUST emit thinking blocks as distinct events, separate from text content blocks.

### Key Entities

- **AnthropicStreamFn**: The streaming function that connects to the Anthropic messages endpoint and produces assistant message events.
- **Thinking Configuration**: Budget control for extended thinking, specifying maximum tokens for model reasoning.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Text responses stream incrementally — each delta arrives as a separate event, not buffered until completion.
- **SC-002**: Tool calls produce valid, parseable JSON arguments upon completion.
- **SC-003**: Thinking blocks are emitted as events distinct from text content when thinking is enabled.
- **SC-004**: All Anthropic error codes map to the correct agent error types consistently.

## Clarifications

### Session 2026-03-20

- Q: Is 529 overloaded retryable? → A: Yes, classified as `error_network` (retryable).
- Q: Thinking block exceeds budget? → A: Provider enforces budget; adapter passes through.
- Q: Stream interrupted mid-thinking? → A: Accumulation returns "no terminal event" error.
- Q: Empty tool call arguments? → A: Empty string → `{}` per core rules.
- Q: Unrecognized content block type? → A: Silently skipped.

## Assumptions

- The Anthropic messages endpoint uses Server-Sent Events for streaming.
- The shared conversion trait and error classifier from the adapter shared infrastructure (spec 011) are available.
- Thinking block support requires specific model versions that support extended thinking.
- The adapter does not manage API key storage — credentials are provided by the caller.

## Addendum: FR-006 Retry-After Implementation Notes (2026-07-06)

FR-006 was previously unimplemented — the adapter classified HTTP errors but never read response headers, so no retry-after hint was ever surfaced. This addendum records the design used to close that gap:

- `AssistantMessageEvent::Error` (core crate, `src/stream.rs`) gained a new field, `retry_after: Option<std::time::Duration>`. `None` means the provider gave no hint (the default for every existing error constructor and for every other adapter); `Some(duration)` means a hint was parsed from the response.
- `adapters/src/classify.rs` gained two additive helpers: `parse_retry_after(&reqwest::header::HeaderMap) -> Option<Duration>` (and its testable inner `parse_retry_after_value(&str)`), and `with_retry_after(event, Option<Duration>) -> AssistantMessageEvent` to merge a parsed hint onto an already-classified error event. Both are shared infrastructure other adapters can adopt.
- Header forms supported (RFC 9110 §10.2.3): **delay-seconds** (e.g. `"30"`) and **HTTP-date** (e.g. `"Wed, 21 Oct 2026 07:28:00 GMT"`, parsed via `chrono::DateTime::parse_from_rfc2822` and converted to a duration from now, clamped to zero if already past). Unparseable values return `None`.
- Wiring is Anthropic-only per FR-006's scope: `adapters/src/anthropic.rs` reads the `Retry-After` header from the HTTP response (before the body is consumed) whenever the response status is non-2xx, and attaches it to the classified event regardless of which status produced it (429, 529, 504, or any other non-success code). In practice only 429 (and occasionally 529) responses carry the header from Anthropic. Mid-stream SSE `error` events (which arrive after a 200 OK, once the stream is already established) have no HTTP headers to read and always carry `retry_after: None`.
- **Consumption**: the retry call site in `src/loop_/stream.rs` (`handle_stream_error`) prefers a provider-supplied `retry_after` hint over `RetryStrategy::delay`'s computed backoff when the hint is present; the retry *decision* (`should_retry`) remains the strategy's alone, and `RetryStrategy` implementations themselves do not see the hint.
