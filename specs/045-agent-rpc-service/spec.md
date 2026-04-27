# Feature Specification: JSON-RPC Agent Service

**Feature Branch**: `045-agent-rpc-service`
**Created**: 2026-04-27
**Status**: Draft
**Input**: A new workspace crate `swink-agent-rpc` that exposes a `swink_agent::Agent` over a Unix-domain socket using JSON-RPC 2.0 / NDJSON. Includes a server that hosts an agent behind a Unix socket with peer-credential security, a Rust client that provides an agent-shaped streaming API, and a `swink-agentd` daemon binary. External processes (CLIs, IDE plugins, evaluation harnesses, third-party clients) can drive the agent loop without embedding Rust code. The TUI continues to use its existing in-process transport unchanged. Single-tenant, per-user daemon model — not a multi-client service. Unix-only on the wire; workspace still builds on Windows. Supersedes spec 025 FR-015 SocketTransport stub which has been removed.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Connect and Send a Prompt (Priority: P1)

A developer building an IDE plugin wants to send a text prompt to a running agent and receive the full response. They connect to the daemon's socket, send a prompt containing their question, and receive a stream of agent events (thinking steps, tool calls, text output) followed by a turn completion signal. The client collects all events and returns them as a batch.

**Why this priority**: Sending prompts and receiving events is the fundamental interaction. Without this, the RPC layer provides no value. This single story delivers a complete, usable agent interface for any external process.

**Independent Test**: Can be tested by starting a server with a mock agent that returns a fixed response, connecting a client, sending "Hello", and verifying the returned events include the expected text content and a turn completion identifier.

**Acceptance Scenarios**:

1. **Given** a running server and a connected client, **When** the client sends a text prompt, **Then** the client receives one or more agent event notifications followed by a prompt completion response containing a turn identifier.
2. **Given** a prompt that triggers the agent to produce multiple content blocks (thinking + text), **When** the events are collected, **Then** they arrive in the same order the agent produced them.
3. **Given** a client that connects to a socket where no server is running, **When** the connection is attempted, **Then** the client receives a clear connection error.
4. **Given** a server with a mock agent, **When** multiple sequential prompts are sent on the same connection, **Then** each prompt produces its own set of events and turn identifier.

---

### User Story 2 - Handle Tool Approval Requests (Priority: P1)

A security-conscious operator configures the agent with tools that require explicit approval before execution. When the agent decides to call such a tool, the server sends an approval request to the connected client. The client's approval handler inspects the tool name and arguments, decides whether to approve or reject, and sends the decision back. The agent either proceeds with the tool call or skips it based on the response.

**Why this priority**: Tool approval is a safety-critical path. Without it, either all tools must be auto-approved (unsafe) or tools requiring approval cannot be used through the RPC interface. This must work before deployment.

**Independent Test**: Can be tested by configuring an agent with a tool that requires approval, connecting a client with an approval handler that conditionally approves, sending a prompt that triggers the tool, and verifying the handler is called with the correct tool name and arguments and the agent respects the decision.

**Acceptance Scenarios**:

1. **Given** a client with an approval handler and an agent that calls a tool requiring approval, **When** the tool call is initiated, **Then** the server sends a tool approval request to the client containing the tool name, arguments, and context.
2. **Given** an approval handler that returns "approved", **When** the server receives the response, **Then** the agent proceeds to execute the tool.
3. **Given** an approval handler that returns "rejected", **When** the server receives the response, **Then** the agent skips the tool call and continues the turn.
4. **Given** a client with no approval handler set, **When** a tool approval request is received, **Then** the tool is auto-approved.

---

### User Story 3 - Start and Stop the Agent Daemon (Priority: P1)

An operator wants to host an agent behind a well-known socket path so that external tools can connect. They run the daemon with a socket path and model name. The daemon starts, binds the socket, restricts file permissions so only the owning user can connect, and waits for clients. When the operator sends a shutdown signal, the daemon shuts down gracefully and removes the socket file.

**Why this priority**: The server is the foundation for all remote interactions. Without a running daemon, no external process can drive the agent.

**Independent Test**: Can be tested by starting the daemon, verifying the socket file exists with expected permissions, connecting with a test client to confirm the handshake completes, then sending a shutdown signal and confirming the socket file is cleaned up.

**Acceptance Scenarios**:

1. **Given** a valid socket path and model name, **When** the daemon starts, **Then** the socket file is created with permissions restricted to the owning user only.
2. **Given** a daemon that is running, **When** the operator sends a shutdown signal, **Then** the daemon shuts down, active sessions are terminated, and the socket file is removed.
3. **Given** a socket path where a file already exists, **When** the daemon starts without force mode, **Then** it refuses to start and reports the conflict.
4. **Given** a socket path where a stale file exists, **When** the daemon starts with force mode, **Then** it removes the existing file and binds successfully.

---

### User Story 4 - Reject Unauthorized Connections (Priority: P1)

A multi-user system runs several agent daemons. Each user's daemon listens on a socket in their home directory. When a process running as a different user attempts to connect, the daemon checks the peer's credentials and rejects the connection before any handshake occurs. Only processes running as the same effective user are permitted.

**Why this priority**: Security is mandatory. Without peer-credential checking, any local user could interact with another user's agent, potentially exfiltrating data or executing tools on their behalf.

**Independent Test**: Can be tested by verifying the server checks peer credentials after accepting a connection and before proceeding with the handshake. Socket file permissions provide the first layer; peer-credential checking provides a second defense-in-depth layer.

**Acceptance Scenarios**:

1. **Given** a connection from a process with the same effective user ID, **When** the peer credential check runs, **Then** the connection is accepted and the handshake proceeds.
2. **Given** a connection from a process with a different effective user ID, **When** the peer credential check runs, **Then** the connection is rejected before the handshake.
3. **Given** a platform where peer credential checking is not supported, **When** a connection arrives, **Then** the server logs a warning and allows the connection (falling back to socket permissions as the only access control).

---

### User Story 5 - Cancel a Running Prompt (Priority: P2)

A developer realizes they sent the wrong prompt and wants to cancel the current turn before it completes. They send a cancel signal through the client. The server signals the agent to stop at the next safe boundary. Any remaining events up to the cancellation point are delivered, and the turn ends.

**Why this priority**: Cancellation is important for user experience and cost control, but the system functions without it — prompts simply run to completion.

**Independent Test**: Can be tested by starting a long-running prompt (agent configured with a slow mock), sending a cancel signal after the first event, and verifying the turn ends before all events would have been produced.

**Acceptance Scenarios**:

1. **Given** a running prompt, **When** the client sends a cancel signal, **Then** the agent stops at the next safe boundary and the turn ends.
2. **Given** a cancel signal sent before any events are produced, **When** the agent processes it, **Then** the turn ends with zero or minimal events.
3. **Given** a cancel signal sent after the turn has already completed, **When** the server receives it, **Then** the signal is harmlessly ignored.

---

### User Story 6 - Enforce Single-Session Access (Priority: P2)

An operator has one client connected to the daemon. A second client (perhaps a stale process or accidental double-launch) attempts to connect. The server detects that a session is already active and rejects the second connection with a clear "session in use" error. The first client's session is unaffected.

**Why this priority**: Single-session enforcement prevents resource conflicts and confused state from concurrent agent interactions, but the system is usable without it if only one client connects at a time.

**Independent Test**: Can be tested by connecting two clients concurrently and verifying the second receives a "session in use" error while the first continues operating normally.

**Acceptance Scenarios**:

1. **Given** one active client connection, **When** a second client connects, **Then** the second receives a "session in use" error and is disconnected.
2. **Given** the first client disconnects, **When** a new client connects, **Then** the connection is accepted and a new session begins.
3. **Given** a second connection is rejected, **When** the first client continues operating, **Then** the first client's events and requests flow normally without interruption.

---

### Edge Cases

- What happens when the client disconnects mid-prompt? The server detects the broken connection, aborts the agent turn, and makes the session available for a new connection.
- What happens when the server receives a malformed message? The message is logged and dropped; the connection remains open for valid messages.
- What happens when a single message exceeds the maximum size (1 MiB)? The connection is closed to prevent memory exhaustion.
- What happens when the client sends a request with an unknown method? The server responds with a standard "method not found" error.
- What happens when the server shuts down while a prompt is in progress? The agent turn is aborted, remaining events are flushed, and the client's pending request resolves with a disconnection error.
- What happens on a non-Unix platform? The server and client return descriptive "not available" errors. The TUI's in-process transport is unaffected.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The crate MUST be a new workspace member that depends on the core agent crate's public APIs only.
- **FR-002**: The crate MUST declare `#[forbid(unsafe_code)]` at the crate root. Its `Cargo.toml` MUST define feature gates (`client`, `server`, `cli`) consistent with the workspace pattern. Default features are `client` and `server`.
- **FR-003**: The wire protocol MUST be newline-delimited JSON with one compact message per line. Maximum line length is 1 MiB. Lines exceeding the limit MUST cause the connection to close.
- **FR-004**: The handshake MUST consist of: client sends an `initialize` notification with protocol version and client identity; server responds with an `initialized` notification with protocol version and server identity. Version mismatch MUST result in connection closure.
- **FR-005**: The client MUST be able to send a `prompt` request containing a text string. The server MUST stream event notifications for each agent event and respond to the request with a turn identifier when the turn completes.
- **FR-006**: The server MUST support tool approval requests sent to the client when the agent invokes a tool that requires approval. The client MUST be able to respond with approved, rejected, or approved-with-modification.
- **FR-007**: The client MUST be able to send a `cancel` notification to abort the current turn at the next safe boundary.
- **FR-008**: The client MUST be able to send a `shutdown` notification to end the session and close the connection.
- **FR-009**: The server MUST bind the Unix socket with permissions restricted to the owning user (mode 0600).
- **FR-010**: The server MUST verify peer credentials on each accepted connection. On supported platforms, connections from a different effective user ID MUST be rejected. On unsupported platforms, a warning MUST be logged.
- **FR-011**: The server MUST enforce single-session access. A second concurrent connection MUST be rejected with a "session in use" error.
- **FR-012**: The server MUST remove the socket file on shutdown (both graceful and signal-driven).
- **FR-013**: The daemon binary MUST accept command-line arguments for socket path, model, system prompt, and a force flag to remove existing socket files.
- **FR-014**: On non-Unix platforms, the server and client MUST provide stub implementations that return descriptive "not available" errors. The workspace MUST continue to build on all supported platforms.
- **FR-015**: The transport layer MUST be generic over any async reader/writer pair, not coupled to Unix sockets. This enables future transports without modifying the peer.

### Key Entities

- **Agent Server**: The server that binds a Unix socket, accepts connections, and hosts agent sessions. Configured with a factory that creates fresh agent configurations per session.
- **Agent Client**: The client that connects to a server, sends prompts, and receives agent events. Supports an optional approval handler for tool safety.
- **Transport Peer / Sender**: Transport-agnostic message peer with reader and writer tasks. The sender handle is cloneable for use in callbacks and concurrent operations.
- **Wire DTOs**: Protocol data transfer objects that bridge between core agent types and the wire format, decoupling the wire schema from internal type evolution.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A client can connect, complete the handshake, send a prompt, receive agent events, and shut down — the full lifecycle — against a running server with a mock agent within 5 seconds end-to-end.
- **SC-002**: Tool approval round-trips complete correctly: server sends approval request, client responds, agent respects the decision — verified with both approved and rejected scenarios in under 2 seconds per round-trip.
- **SC-003**: Cancellation stops agent turns at the next boundary — a cancelled prompt produces fewer events than an uncancelled one, verified by comparing event counts.
- **SC-004**: Socket permissions are restricted to the owning user after server bind, verified on both supported Unix platforms.
- **SC-005**: A second concurrent client receives a "session in use" error within 1 second of connecting.
- **SC-006**: Peer credential checks reject connections from different user IDs on supported platforms.
- **SC-007**: The workspace builds successfully on all supported platforms (Linux, macOS, Windows) — non-Unix platforms compile the stub code paths without errors.
- **SC-008**: All wire messages are valid per the protocol specification — verified by round-trip serialization/deserialization tests with 100% coverage of message types.
- **SC-009**: The server cleans up the socket file on both graceful shutdown and signal-driven shutdown.

## Assumptions

- The existing core agent types (`Agent`, `AgentOptions`, `AgentEvent`, `ToolApproval`, `ToolApprovalRequest`) are stable and publicly re-exported from the core crate.
- The core agent's prompt stream returns a stream that does not borrow the agent, allowing concurrent abort calls while the stream is in flight.
- The core agent options support an async approval callback that can be wired before agent construction.
- The `nix` crate (already in workspace dependencies) provides safe wrappers for peer credential checks on Linux and macOS.
- The TUI's in-process transport is unaffected by this work. The TUI does not depend on or consume this crate.
- The daemon is a single-tenant, per-user process. Multi-client and multi-tenant scenarios are out of scope.
- Authentication beyond peer-credential matching (tokens, TLS, certificates) is out of scope.
- Windows named-pipe and HTTP/SSE transports are out of scope.
- Reconnection and resumable sessions are out of scope.
