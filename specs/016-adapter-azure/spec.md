# Feature Specification: Adapter: Azure OpenAI

**Feature Branch**: `016-adapter-azure`
**Created**: 2026-03-20
**Status**: Draft
**Input**: AzureStreamFn for Azure OpenAI endpoints via SSE. Deployment-oriented routing. Azure-specific auth and API versioning. References: PRD §15.1, HLD Adapters.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Stream Text Responses from Azure OpenAI (Priority: P1)

A developer configures the Azure adapter with a resource endpoint, deployment name, API key, and API version and sends a conversation to the Azure OpenAI chat completions endpoint. The adapter streams back text content in real time as Server-Sent Events, delivering each text delta to the agent loop as it arrives.

**Why this priority**: Streaming text is the fundamental capability — without it, the adapter has no value.

**Independent Test**: Can be tested by sending a simple prompt to an Azure OpenAI deployment and verifying that text deltas arrive incrementally and the final assembled message is coherent.

**Acceptance Scenarios**:

1. **Given** valid Azure credentials, a deployment name, and an API version, **When** a conversation is sent, **Then** text content streams back incrementally via SSE.
2. **Given** a streaming response, **When** all deltas have arrived, **Then** the assembled message matches what the deployment produced.
3. **Given** a streaming response, **When** the stream ends with a `[DONE]` sentinel, **Then** a terminal event signals completion.

---

### User Story 2 - Stream Tool Call Responses from Azure OpenAI (Priority: P1)

A developer sends a conversation with tool definitions to the Azure OpenAI deployment. The adapter streams back tool call chunks, including the tool name, tool call ID, and incrementally-arriving JSON arguments. The agent loop receives structured tool call events that it can dispatch for execution.

**Why this priority**: Tool calling is essential for agentic workflows — the primary use case of this library.

**Independent Test**: Can be tested by sending a prompt with tool definitions that the model is likely to invoke, and verifying that tool call events arrive with correct names, IDs, and parseable arguments.

**Acceptance Scenarios**:

1. **Given** a conversation with tool definitions, **When** the model decides to call a tool, **Then** the adapter emits tool call start, delta, and end events.
2. **Given** streamed tool call arguments, **When** the tool call ends, **Then** the accumulated arguments form valid JSON.
3. **Given** a response with multiple parallel tool calls, **When** streamed, **Then** each tool call is emitted as a separate indexed block with its own ID.

---

### User Story 3 - Route Requests to Azure Deployments with API Versioning (Priority: P2)

A developer configures the Azure adapter with a resource endpoint and deployment name rather than a generic base URL. The adapter constructs the correct Azure-specific URL path that includes the deployment name and API version as a query parameter. This deployment-oriented routing is distinct from standard OpenAI routing, where the model is specified in the request body.

**Why this priority**: Deployment routing and API versioning are what distinguish Azure from standard OpenAI, but the streaming protocol itself is similar.

**Independent Test**: Can be tested by verifying that the adapter constructs the correct URL from the resource endpoint, deployment name, and API version.

**Acceptance Scenarios**:

1. **Given** a resource endpoint and deployment name, **When** a request is made, **Then** the URL includes the deployment name in the path.
2. **Given** an API version, **When** a request is made, **Then** the API version is included as a query parameter.
3. **Given** Azure credentials, **When** a request is made, **Then** the authentication header uses the Azure-specific key header.

---

### User Story 4 - Handle Errors from Azure OpenAI (Priority: P2)

A developer encounters various error conditions when communicating with the Azure OpenAI endpoint (invalid key, rate limiting, deployment not found, content filter violations, server errors, network timeouts). The adapter classifies these errors using the shared error classifier so that the agent loop can apply appropriate retry strategies.

**Why this priority**: Correct error handling enables reliable operation, but the adapter can demonstrate value with the happy path alone.

**Independent Test**: Can be tested by simulating error responses (429, 401, 404, 500, network timeout) and verifying each maps to the correct error type.

**Acceptance Scenarios**:

1. **Given** an HTTP 429 response from Azure, **When** classified, **Then** it maps to a rate-limit error (retryable) with retry-after timing if provided.
2. **Given** an HTTP 401 response, **When** classified, **Then** it maps to an authentication error (not retryable).
3. **Given** a deployment-not-found (404) error, **When** classified, **Then** it maps to a non-retryable error.
4. **Given** a content filter violation, **When** classified, **Then** it maps to an appropriate error type.

---

### Edge Cases

- What happens when the specified deployment does not exist — is the 404 error clearly distinguished from other 404s?
- How does the adapter handle Azure content filter violations that block a response mid-stream?
- What happens when the API version is not supported by the deployment?
- How does the adapter handle Azure-specific rate limiting headers (which may differ from standard OpenAI)?
- What happens when the Azure resource endpoint URL has a trailing slash?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The adapter MUST stream text responses from the Azure OpenAI chat completions endpoint via SSE, emitting incremental text deltas.
- **FR-002**: The adapter MUST stream tool call responses, emitting tool name, tool call ID, argument deltas, and completion events.
- **FR-003**: The adapter MUST construct deployment-oriented URLs from the resource endpoint and deployment name.
- **FR-004**: The adapter MUST include the API version as a query parameter on all requests.
- **FR-005**: The adapter MUST use Azure-specific authentication headers.
- **FR-006**: The adapter MUST convert agent messages to the OpenAI chat completions format using the shared conversion trait.
- **FR-007**: The adapter MUST classify HTTP errors using the shared error classifier (429 → rate limit, 401/403 → auth, 404 → non-retryable, 5xx → network, timeout → network).

### Key Entities

- **AzureStreamFn**: The streaming function that connects to an Azure OpenAI deployment and produces assistant message events.
- **Deployment Configuration**: The combination of resource endpoint, deployment name, and API version that identifies an Azure OpenAI deployment.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Text responses stream incrementally — each delta arrives as a separate event, not buffered until completion.
- **SC-002**: Tool calls produce valid, parseable JSON arguments upon completion.
- **SC-003**: Deployment-oriented URLs are correctly constructed from resource endpoint, deployment name, and API version.
- **SC-004**: All Azure error codes map to the correct agent error types consistently.

## Assumptions

- Azure OpenAI uses the same SSE streaming protocol and message format as standard OpenAI, but with different URL routing and authentication.
- The shared conversion trait and error classifier from the adapter shared infrastructure (spec 011) are available.
- The adapter does not manage API key storage — credentials are provided by the caller.
- Azure content filter violations are treated as error conditions.
