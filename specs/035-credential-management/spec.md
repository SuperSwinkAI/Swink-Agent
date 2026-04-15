# Feature Specification: OAuth2 & Credential Management

**Feature Branch**: `035-credential-management`
**Created**: 2026-03-31
**Status**: Draft
**Input**: User description: "OAuth2 & credential management — pluggable credential storage, automatic token lifecycle, and authenticated tool execution for the agent framework"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Tool Uses API Key Credential (Priority: P1)

A library consumer builds an agent with a tool that calls an external API requiring an API key. They seed the in-memory credential store at agent instantiation with the API key under a well-known key name. The tool declares its auth requirement, and the framework resolves the credential and makes it available to the tool before execution — the tool never manages credential lookup itself.

**Why this priority**: API key authentication is the simplest and most common credential type. It establishes the core credential store, tool auth declaration, and resolution pipeline without the complexity of token expiry or refresh.

**Independent Test**: Can be fully tested by seeding an in-memory credential store with a test API key at agent construction, adding a tool that declares an auth requirement, and verifying the tool receives the resolved credential during execution.

**Acceptance Scenarios**:

1. **Given** a credential store containing an API key under "weather-api", **When** a tool with auth config pointing to "weather-api" executes, **Then** the tool receives the resolved API key credential before making its request.
2. **Given** a credential store with no entry for "weather-api", **When** a tool requiring "weather-api" credential executes, **Then** the tool receives a clear error indicating the credential is missing, and the tool execution is skipped with an error result sent to the LLM.
3. **Given** a tool with no auth requirement, **When** it executes, **Then** no credential resolution occurs and execution proceeds normally (zero overhead for unauthenticated tools).

---

### User Story 2 - Bearer Token with Automatic Expiry Check (Priority: P1)

A library consumer builds an agent with a tool that calls an API using bearer token authentication. The stored credential includes an expiration timestamp. The framework checks expiry before each tool execution and rejects expired tokens with a clear error rather than allowing a request that will fail with a 401.

**Why this priority**: Bearer tokens with expiry are the second most common pattern (after API keys) and introduce the critical concept of credential validity checking, which is foundational for OAuth2 refresh.

**Independent Test**: Can be fully tested by storing a bearer token with a past expiration, executing the tool, and verifying an expiry error is returned without the tool executing.

**Acceptance Scenarios**:

1. **Given** a bearer token with expiry 1 hour from now, **When** the tool executes, **Then** the credential is resolved successfully and the tool receives the token.
2. **Given** a bearer token that expired 5 minutes ago, **When** the tool executes, **Then** the credential resolution returns an expiry error and the tool is not executed.
3. **Given** a bearer token with no expiry timestamp, **When** the tool executes, **Then** the token is treated as valid (no expiry check).

---

### User Story 3 - OAuth2 Token Auto-Refresh (Priority: P2)

A library consumer builds an agent with a tool that uses OAuth2 credentials (e.g., Google Calendar API). The stored credential includes an access token, refresh token, and expiry. When the access token expires, the framework automatically refreshes it using the refresh token, updates the credential store with the new tokens, and proceeds with tool execution — all transparent to the tool.

**Why this priority**: OAuth2 refresh is the highest-value automated behavior. Without it, every tool that uses OAuth2 must implement its own refresh logic, which is error-prone and duplicative.

**Independent Test**: Can be fully tested by storing an expired OAuth2 credential with a valid refresh token, mocking the token endpoint, and verifying the refresh occurs, the store is updated, and the tool receives the new access token.

**Acceptance Scenarios**:

1. **Given** an OAuth2 credential with an expired access token and a valid refresh token, **When** the tool executes, **Then** the framework refreshes the access token, stores the new tokens, and the tool receives the fresh access token.
2. **Given** an OAuth2 credential with an expired access token and the refresh also fails (e.g., refresh token revoked), **When** the tool executes, **Then** an error is returned indicating the credential could not be refreshed, and the tool is not executed.
3. **Given** two tools executing concurrently that both need the same expired OAuth2 credential, **When** both trigger refresh simultaneously, **Then** only one refresh request is made (deduplication) and both tools receive the new token.

---

### User Story 4 - OAuth2 Initial Authorization Flow (Priority: P2)

A library consumer deploys an agent that needs to access a user's Google Calendar for the first time. No OAuth2 tokens exist yet. The framework initiates the authorization code flow: it constructs the authorization URL, delegates the user interaction (browser open, callback listen) to a configurable handler, exchanges the authorization code for tokens, and stores them. Subsequent tool executions use the stored tokens.

**Why this priority**: Initial authorization is required before refresh can work. It completes the OAuth2 lifecycle but is more complex due to user interaction requirements.

**Independent Test**: Can be fully tested by mocking the authorization handler (simulating a user completing the flow) and verifying tokens are stored after the exchange.

**Acceptance Scenarios**:

1. **Given** no stored credential for "google-calendar", **When** a tool requiring "google-calendar" credential executes and the authorization handler is configured, **Then** the framework initiates the authorization flow, the handler is invoked with the authorization URL, and after the user completes authorization, tokens are stored.
2. **Given** no stored credential and no authorization handler configured, **When** a tool requiring the credential executes, **Then** an error is returned indicating the credential is missing and no authorization handler is available.
3. **Given** the authorization flow is initiated but the user does not complete it within a timeout period, **When** the timeout elapses, **Then** the tool execution fails with a timeout error.

---

### User Story 5 - Headless Deployment with Pre-Provisioned Credentials (Priority: P3)

A library consumer deploys an agent in a headless server environment where no browser or user interaction is possible. All credentials are pre-provisioned in the in-memory credential store at agent instantiation. OAuth2 tokens are pre-authorized and seeded before the agent starts. The framework must not attempt interactive authorization in headless mode.

**Why this priority**: Headless deployment is essential for production but can be supported simply by not configuring an authorization handler. Pre-provisioned credentials already work with the store trait.

**Independent Test**: Can be fully tested by running an agent with pre-provisioned credentials and no authorization handler, verifying tools execute using stored credentials and no interactive flow is attempted.

**Acceptance Scenarios**:

1. **Given** a headless deployment with pre-provisioned OAuth2 credentials, **When** tools execute, **Then** credentials are resolved from the store and refresh works automatically.
2. **Given** a headless deployment with expired credentials and no refresh token, **When** a tool executes, **Then** an error is returned (no interactive fallback attempted).

---

### Edge Cases

- What happens when a custom credential store's underlying storage becomes unavailable? The store returns an error, which propagates as a credential resolution error. The tool execution is skipped with an error result sent to the LLM. The agent loop continues.
- What happens when credential resolution is slow (e.g., custom store with network call)? Resolution is async with a configurable timeout (default: 30 seconds). Timeout triggers an error result.
- What happens when a tool declares an auth scheme that does not match the stored credential type (e.g., tool expects bearer but store has API key)? The credential is resolved but the scheme mismatch is reported as an error. The tool is not executed.
- What happens when the same credential key is used by multiple tools and one tool triggers a refresh? The refresh is deduplicated — only one refresh request is made. All tools waiting on the same credential receive the result.
- What happens when credentials contain secrets in logs or error messages? Credential values MUST never appear in log output, error messages, or event payloads. Only credential keys (names) and metadata (expiry, type) may be logged.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST provide a pluggable credential storage abstraction that supports get, set, and delete operations by string key.
- **FR-002**: System MUST support at least three credential types: API key (a single secret string), bearer token (token string with optional expiry), and OAuth2 (access token, refresh token, expiry, token endpoint URL, client identifier, optional client secret, scopes).
- **FR-003**: System MUST provide a built-in in-memory credential store, seeded at agent instantiation via configuration. This is the only built-in store; consumers may implement the trait for custom backends.
- **FR-007**: System MUST provide a credential resolver that checks credential validity (expiry) before returning it to the requesting tool.
- **FR-008**: System MUST automatically refresh expired OAuth2 credentials using the stored refresh token, without tool involvement.
- **FR-009**: When multiple concurrent requests need to refresh the same credential, the system MUST deduplicate to a single refresh request and share the result. **Single-flight token refresh**: The credential resolver deduplicates concurrent refresh requests using a `SingleFlightTokenSource` wrapper. When multiple callers request the same expired credential simultaneously, only one outbound refresh HTTP call is made; all callers share the same resulting future and receive the token together. This prevents thundering-herd token refreshes when many tool executions start concurrently.
- **FR-010**: System MUST support an authorization handler abstraction for initiating interactive OAuth2 authorization code flows (browser-based).
- **FR-011**: When no authorization handler is configured and a credential is missing, the system MUST return a clear error without attempting interactive authorization.
- **FR-012**: System MUST integrate with the tool dispatch pipeline so that tools declaring auth requirements have credentials resolved before execution.
- **FR-012a**: The `AgentTool` trait MUST be extended with an `auth_config()` method that returns an optional `AuthConfig`. The default implementation MUST return `None` (no auth required), preserving backward compatibility for existing tools.
- **FR-012b**: The `AgentTool::execute` method MUST receive the resolved credential as an `Option<ResolvedCredential>` parameter. Tools with no auth requirement receive `None`.
- **FR-013**: Tools without auth requirements MUST experience zero overhead from the credential system.
- **FR-014**: Credential resolution MUST be async with a configurable timeout.
- **FR-015**: If credential resolution fails (missing, expired without refresh, store error), the tool MUST NOT execute. An error result MUST be sent to the LLM describing the failure.
- **FR-016**: Credential values (tokens, keys, secrets) MUST never appear in log output, error messages, event payloads, or tracing spans. Only credential keys (names), types, and metadata (expiry timestamps) may be logged.
- **FR-017**: The credential store abstraction MUST be thread-safe for concurrent access from multiple tool executions.
- **FR-018**: System MUST support a credential type mismatch check — if a tool expects a bearer token but the stored credential is an API key, an error MUST be reported before tool execution.
- **FR-019**: The entire credential management feature MUST be optional — agents that do not configure a credential store or resolver MUST behave identically to pre-035 agents.
- **FR-020**: The authorization code flow MUST support a configurable timeout for user completion (default: 5 minutes).
- **FR-021**: After a successful OAuth2 token refresh, the updated credential MUST be written back to the credential store so subsequent executions use the new token.
- **FR-022**: The credential resolver MUST treat bearer tokens without an expiry timestamp as perpetually valid (no expiry check).
- **FR-023**: The credential resolver MUST apply a configurable expiry buffer (default: 60 seconds) — credentials expiring within the buffer period are treated as expired and refreshed proactively.
- **FR-024**: **ToolMiddleware auth and metadata delegation**: When a tool is wrapped in `ToolMiddleware`, the middleware MUST delegate `auth_config()` and `metadata()` calls to the inner tool unchanged. Without this delegation, the credential resolver cannot discover what credentials the wrapped tool requires, causing auth to silently fail.

### Key Entities

- **Credential**: A secret value with type information — API key, bearer token, or OAuth2 token set. Includes metadata such as expiry and scopes.
- **CredentialStore**: A pluggable storage trait for credentials. Supports get/set/delete by string key. The framework ships only an in-memory implementation; consumers may implement the trait for custom backends.
- **CredentialResolver**: The orchestrator that checks credential validity, triggers refresh when needed, deduplicates concurrent refreshes, and returns resolved credentials to tools.
- **AuthConfig**: Per-tool configuration declaring which credential key to use and how to attach the credential to the request (bearer header, API key header, API key query parameter).
- **AuthorizationHandler**: A pluggable callback for initiating interactive OAuth2 authorization flows. Receives the authorization URL and returns the authorization code.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A tool can declare an auth requirement and receive a resolved credential before execution with no manual credential lookup code in the tool implementation.
- **SC-002**: Expired OAuth2 tokens are refreshed automatically and transparently — tool implementations do not handle refresh logic.
- **SC-003**: Concurrent tool executions that trigger the same credential refresh result in exactly one outbound refresh request.
- **SC-004**: Agents without credential stores configured behave identically to pre-035 agents (zero regression, zero overhead).
- **SC-005**: Credential secrets never appear in any log output, error message, or event payload at any verbosity level.
- **SC-006**: The authorization code flow can be completed end-to-end (authorization URL generation through token storage) using a configurable handler.
- **SC-007**: The in-memory credential store passes roundtrip tests (get/set/delete) for all three credential types.

## Clarifications

### Session 2026-03-31

- Q: How does a tool declare its auth requirement — trait method, external config, or super-trait? → A: Add `fn auth_config(&self) -> Option<AuthConfig>` to the existing `AgentTool` trait with a default returning `None`.
- Q: Should the credential store manage persistent storage (env vars, keychain, file) or only hold credentials passed at config time? → A: Only in-memory store, seeded at agent instantiation. Drop env var store (FR-004), keychain store (FR-005), and store chaining (FR-006). OAuth2 refresh updates in-memory state for the session lifetime. No framework-managed persistent storage.
- Q: How should the resolved credential be delivered to the tool during execution? → A: Add `credential: Option<ResolvedCredential>` parameter to `AgentTool::execute`. Tools with no auth requirement receive `None`.

## Assumptions

- OAuth2 authorization code flow is the only supported OAuth2 grant type in the initial implementation. Client credentials flow and device code flow are out of scope but the design should not preclude them.
- The authorization handler is a simple callback that receives a URL and returns an authorization code. The framework does not embed a web server or browser automation — these are the handler's responsibility.
- The framework does not manage credential persistence. Credentials are passed in at agent instantiation. The only built-in store is in-memory. Consumers who need persistent storage (env vars, keychain, secrets manager) implement the `CredentialStore` trait themselves.
- The credential management feature lives in a new workspace crate (`swink-agent-auth`) to avoid pulling OAuth2/HTTP dependencies into the core crate, consistent with the library-first principle.
- The expiry buffer (60 seconds) is a reasonable default for most OAuth2 providers. Consumers can override it.
- The existing `get_api_key` callback in AgentOptions is NOT replaced. It continues to serve its purpose (LLM API key resolution). The credential system is for tool-level authentication, which is a different concern.
