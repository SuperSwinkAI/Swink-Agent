# Feature Specification: Adapter: AWS Bedrock

**Feature Branch**: `019-adapter-bedrock`
**Created**: 2026-03-20
**Status**: Draft
**Input**: BedrockStreamFn for AWS Bedrock via ConverseStream (AWS event-stream encoding). AWS SigV4 request signing. References: PRD §15.1, HLD Adapters.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Stream Text Responses from AWS Bedrock (Priority: P1)

A developer configures the Bedrock adapter with AWS credentials, a region, and a model ID and sends a conversation to the Bedrock streaming endpoint. The adapter streams back text content in real time via the ConverseStream API (AWS event-stream encoding), delivering each text delta to the agent loop as it arrives. The developer sees assistant responses appear incrementally.

**Why this priority**: Streaming text is the fundamental capability — without it, the adapter has no value.

**Independent Test**: Can be tested by sending a simple prompt to a Bedrock endpoint and verifying that text deltas arrive incrementally and the final assembled message is coherent.

**Acceptance Scenarios**:

1. **Given** valid AWS credentials, a region, and a model ID, **When** a conversation is sent, **Then** text content streams back incrementally via the ConverseStream API.
2. **Given** a streaming response, **When** all deltas have arrived, **Then** the assembled message matches what the model produced.
3. **Given** a streaming response, **When** the stream completes normally, **Then** a terminal event signals completion.

---

### User Story 2 - Stream Tool Call Responses from AWS Bedrock (Priority: P1)

A developer sends a conversation with tool definitions to the Bedrock streaming endpoint. The adapter streams back tool call events, including the tool name and incrementally-arriving JSON arguments. The agent loop receives structured tool call events that it can dispatch for execution.

**Why this priority**: Tool calling is essential for agentic workflows — the primary use case of this library.

**Independent Test**: Can be tested by sending a prompt with tool definitions that the model is likely to invoke, and verifying that tool call events arrive with correct names and parseable arguments.

**Acceptance Scenarios**:

1. **Given** a conversation with tool definitions, **When** the model decides to call a tool, **Then** the adapter emits tool call start, delta, and end events.
2. **Given** streamed tool call arguments, **When** the tool call ends, **Then** the accumulated arguments form valid JSON.
3. **Given** a response with multiple tool calls, **When** streamed, **Then** each tool call is emitted as a separate indexed block.

---

### User Story 3 - Authenticate with AWS SigV4 Request Signing (Priority: P2)

A developer provides AWS credentials (access key, secret key, and optional session token) and a region. The adapter signs every request using AWS Signature Version 4 so that the request is authenticated by the Bedrock service. The developer does not need to understand the signing process — they provide credentials and the adapter handles the rest.

**Why this priority**: SigV4 signing is what distinguishes Bedrock from other providers — without it, no requests can succeed. However, it is infrastructure rather than user-facing functionality.

**Independent Test**: Can be tested by constructing a request and verifying that the correct SigV4 authorization headers are present and the Bedrock service accepts the request.

**Acceptance Scenarios**:

1. **Given** AWS credentials and a region, **When** a request is made, **Then** the request includes a valid SigV4 authorization header.
2. **Given** temporary credentials with a session token, **When** a request is made, **Then** the session token is included in the signing process.
3. **Given** expired credentials, **When** a request is made, **Then** the adapter surfaces an authentication error (not retryable).

---

### User Story 4 - Handle Errors from AWS Bedrock (Priority: P2)

A developer encounters various error conditions when communicating with the Bedrock endpoint (invalid credentials, throttling, model not available, access denied, server errors, network timeouts). The adapter classifies these errors using the shared error classifier so that the agent loop can apply appropriate retry strategies.

**Why this priority**: Correct error handling enables reliable operation, but the adapter can demonstrate value with the happy path alone.

**Independent Test**: Can be tested by simulating error responses (429, 403, 500, network timeout) and verifying each maps to the correct error type.

**Acceptance Scenarios**:

1. **Given** a throttling response from Bedrock, **When** classified, **Then** it maps to a rate-limit error (retryable).
2. **Given** an access denied response, **When** classified, **Then** it maps to an authentication error (not retryable).
3. **Given** a model-not-available error, **When** classified, **Then** it maps to a non-retryable error.
4. **Given** a network timeout, **When** classified, **Then** it maps to a network error (retryable).

---

### Edge Cases

- AWS credentials expiring mid-stream: the ConverseStream connection is already authenticated at request time, so mid-stream expiry does not affect an in-flight response. Subsequent requests with expired creds surface as auth error (403).
- Bedrock-specific error codes (e.g., `ModelNotReadyException`, `ModelTimeoutException`) are classified by HTTP status code via the shared classifier; the error body is included in the message for diagnostics.
- Model ID not available in the configured region: Bedrock returns HTTP 400 or 404; classified as non-retryable error with descriptive message including region and model ID.
- Bedrock content moderation (`GUARDRAIL_INTERVENED` stop reason) maps to `ContentFiltered` error type, consistent with Azure adapter. Not retryable; distinguishable from auth/network errors.
- SigV4 clock skew (`RequestTimeTooSkewed`, HTTP 403) surfaces as auth error with descriptive message via shared classifier. No auto-retry with corrected timestamp — clock skew is a deployment-environment issue.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The adapter MUST stream text responses from the Bedrock ConverseStream endpoint (AWS event-stream binary encoding), emitting incremental text deltas.
- **FR-002**: The adapter MUST stream tool call responses, emitting tool name, argument deltas, and completion events.
- **FR-003**: The adapter MUST sign all requests using AWS Signature Version 4 with the provided credentials and region.
- **FR-004**: The adapter MUST support temporary credentials (session tokens) in the SigV4 signing process.
- **FR-005**: The adapter MUST convert agent messages to the Bedrock message format using the shared conversion trait.
- **FR-006**: The adapter MUST classify errors using the shared error classifier (throttling → rate limit, access denied → auth, 5xx → network, timeout → network).
- **FR-007**: The adapter MUST construct Bedrock-specific endpoint URLs from the region and model ID.
- **FR-008**: The adapter MUST parse the AWS event-stream binary protocol using the `aws-smithy-eventstream` crate for response framing and CRC validation.

### Key Entities

- **BedrockStreamFn**: The streaming function that connects to the AWS Bedrock streaming endpoint and produces assistant message events.
- **AWS Credentials**: Access key, secret key, optional session token, and region used for SigV4 request signing.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Text responses stream incrementally — each delta arrives as a separate event, not buffered until completion.
- **SC-002**: Tool calls produce valid, parseable JSON arguments upon completion.
- **SC-003**: All requests include valid SigV4 authorization headers accepted by the Bedrock service.
- **SC-004**: All Bedrock error codes map to the correct agent error types consistently.

## Clarifications

### Session 2026-04-02

- Q: Which streaming approach should the adapter use? → A: Streaming `ConverseStream` API with AWS event-stream binary protocol parsing — true incremental delivery, not the non-streaming `Converse` API.
- Q: How should the adapter parse the AWS event-stream binary protocol? → A: Use `aws-smithy-eventstream` crate (AWS's official parser) for binary framing + CRC validation.
- Q: How should Bedrock content moderation blocks be surfaced? → A: Map `GUARDRAIL_INTERVENED` stop reason to `ContentFiltered` error type, consistent with Azure adapter.
- Q: What scope of models should the catalog include? → A: Comprehensive — all available models across all providers on Bedrock (Anthropic, Meta, Mistral, Amazon, AI21, Cohere, etc.).
- Q: How should the adapter handle SigV4 clock skew errors? → A: Surface as auth error with descriptive message — consistent with shared classifier's 403 handling; caller diagnoses.

## Assumptions

- AWS Bedrock uses the ConverseStream API with AWS event-stream binary encoding (`application/vnd.amazon.eventstream`) for streaming — not standard SSE.
- SigV4 signing is required for all Bedrock requests — there is no alternative authentication method.
- The shared conversion trait and error classifier from the adapter shared infrastructure (spec 011) are available.
- The adapter does not manage AWS credential lifecycle (rotation, refresh) — credentials are provided by the caller.
- Bedrock model IDs and region availability are determined by the caller, not by the adapter.
