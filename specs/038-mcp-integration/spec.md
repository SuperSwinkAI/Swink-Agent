# Feature Specification: MCP Integration

**Feature Branch**: `038-mcp-integration`  
**Created**: 2026-04-01  
**Status**: Draft  
**Input**: User description: "MCP Integration — Model Context Protocol client for dynamic tool discovery and execution via external MCP servers"

## Clarifications

### Session 2026-04-01

- Q: Should MCP operations emit events through the existing AgentEvent system? → A: Yes — MCP emits events for connect, disconnect, tool discovery, and tool call forwarded/completed via the existing AgentEvent system.
- Q: How should the agent authenticate to remote SSE MCP servers? → A: Optional bearer token (API key or OAuth2 token) per SSE connection, integrated with the credential resolver (spec 035).
- Q: Can consumers pass environment variables and working directory to MCP subprocess? → A: Command, arguments, and optional environment variable overrides — no working directory config.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Agent Discovers and Uses Tools from an MCP Server (Priority: P1)

A library consumer wants their agent to use tools provided by an external MCP server (e.g., a database query tool, a code search tool, or a file system tool running as a subprocess). They configure the agent with an MCP server connection (command + arguments for stdio transport). When the agent starts, it connects to the MCP server, discovers available tools, and makes them available to the LLM alongside any natively registered tools. The LLM can call MCP-provided tools exactly like native tools — the consumer does not need to write any tool implementation code.

**Why this priority**: This is the core value proposition. Without dynamic tool discovery and execution via MCP, there is no MCP integration. This single story enables the entire MCP ecosystem of tool servers to work with Swink agents.

**Independent Test**: Can be fully tested by starting a mock MCP server (stdio subprocess) that advertises one tool, building an agent that connects to it, and verifying the tool appears in the LLM's tool list and can be called successfully.

**Acceptance Scenarios**:

1. **Given** an agent configured with an MCP server connection (stdio), **When** the agent starts, **Then** the agent connects to the server and discovers all available tools.
2. **Given** an MCP server that advertises a tool named "search_files", **When** the LLM decides to call "search_files" with valid arguments, **Then** the call is forwarded to the MCP server, executed, and the result is returned to the LLM.
3. **Given** an MCP server that returns a text result, **When** the tool call completes, **Then** the result is converted to the agent's standard tool result format and passed back to the LLM as conversation context.
4. **Given** an MCP server that is not running or fails to start, **When** the agent attempts to connect, **Then** a clear error is reported and the agent can continue without the MCP tools (graceful degradation).

---

### User Story 2 - Consumer Connects to Multiple MCP Servers (Priority: P1)

A library consumer wants their agent to connect to multiple MCP servers simultaneously — for example, one server provides database tools, another provides file system tools, and a third provides API integration tools. Each server's tools are namespaced to avoid name collisions. The agent treats all tools uniformly regardless of their source.

**Why this priority**: Real-world agents need multiple tool sources. Without multi-server support and namespace isolation, the integration is limited to trivial single-server use cases.

**Independent Test**: Can be fully tested by starting two mock MCP servers (each advertising a tool with the same name), configuring both with different prefixes, and verifying both tools are available with distinct namespaced names.

**Acceptance Scenarios**:

1. **Given** two MCP servers configured with prefixes "db" and "fs", **When** both advertise a tool named "search", **Then** the agent has two tools: "db_search" and "fs_search".
2. **Given** three MCP servers configured, **When** one server fails to connect, **Then** the other two servers' tools are still available and the failure is logged.
3. **Given** an MCP server configured without a prefix, **When** its tools are discovered, **Then** tools use their original names (no prefix added).

---

### User Story 3 - MCP Tools Participate in Policy and Approval Gates (Priority: P1)

A library consumer has configured approval policies and pre-dispatch policies on their agent. When an MCP-provided tool is called, it goes through the same approval and policy pipeline as native tools. The consumer does not need separate security configuration for MCP tools — the existing policy system applies uniformly.

**Why this priority**: MCP tools execute external code and must be subject to the same security controls as native tools. Without policy integration, MCP tools would be a security bypass.

**Independent Test**: Can be fully tested by configuring an agent with a pre-dispatch deny-list policy that blocks a specific MCP tool name, triggering a call to that tool, and verifying it is blocked.

**Acceptance Scenarios**:

1. **Given** an agent with a tool approval function and an MCP tool that requires approval, **When** the LLM calls the MCP tool, **Then** the approval gate fires before the call is forwarded to the MCP server.
2. **Given** a pre-dispatch policy that denies tools matching a pattern, **When** an MCP tool matches that pattern, **Then** the tool call is rejected with a policy violation message.
3. **Given** a sandbox policy that restricts file path arguments, **When** an MCP tool is called with a blocked path argument, **Then** the policy blocks the call.

---

### User Story 4 - Consumer Filters Which MCP Tools Are Exposed (Priority: P2)

A library consumer connects to an MCP server that advertises many tools, but only wants a subset available to the LLM. They configure allow-list or deny-list filters per MCP server connection. Only tools matching the filter are registered with the agent.

**Why this priority**: Tool filtering reduces prompt size and prevents the LLM from being overwhelmed by too many tools. It is important for production use but not required for basic functionality.

**Independent Test**: Can be fully tested by configuring an MCP server with an allow-list of one tool name, verifying only that tool is registered, and confirming others are excluded.

**Acceptance Scenarios**:

1. **Given** an MCP server with 10 tools and an allow-list of ["tool_a", "tool_b"], **When** tools are discovered, **Then** only "tool_a" and "tool_b" are registered with the agent.
2. **Given** an MCP server with 10 tools and a deny-list of ["tool_c"], **When** tools are discovered, **Then** all tools except "tool_c" are registered.
3. **Given** both an allow-list and deny-list configured, **When** tools are discovered, **Then** the allow-list is applied first, then the deny-list filters the result.

---

### User Story 5 - Agent Connects to Remote MCP Servers via SSE (Priority: P2)

A library consumer wants to connect to a remote MCP server over HTTP using Server-Sent Events (SSE) transport. They provide a URL instead of a command. The agent connects, discovers tools, and executes calls over the network — identical behavior to stdio but over HTTP.

**Why this priority**: SSE transport enables remote and cloud-hosted MCP servers. It extends reach beyond local subprocesses but is not required for initial adoption since most MCP servers support stdio.

**Independent Test**: Can be fully tested by running a local HTTP server that implements MCP over SSE, connecting the agent, and verifying tool discovery and execution work over HTTP.

**Acceptance Scenarios**:

1. **Given** an MCP server URL configured with SSE transport, **When** the agent starts, **Then** it connects via HTTP and discovers available tools.
2. **Given** an active SSE connection, **When** the LLM calls a tool, **Then** the request is sent over HTTP and the response is received via the SSE stream.
3. **Given** an SSE connection that drops, **When** the agent detects the disconnection, **Then** it attempts to reconnect automatically.

---

### User Story 6 - MCP Server Lifecycle Is Managed Automatically (Priority: P2)

A library consumer configures an MCP server that runs as a subprocess (stdio transport). When the agent starts, the subprocess is spawned. When the agent is cancelled or dropped, the subprocess is terminated. If the subprocess crashes mid-conversation, the agent detects the failure and marks those tools as unavailable.

**Why this priority**: Proper lifecycle management prevents zombie processes and resource leaks. Without it, consumers must manage subprocess lifecycle manually, which is error-prone.

**Independent Test**: Can be fully tested by starting an agent with an MCP subprocess, dropping the agent, and verifying the subprocess is terminated.

**Acceptance Scenarios**:

1. **Given** an MCP server configured as a subprocess, **When** the agent starts, **Then** the subprocess is spawned and the connection is established.
2. **Given** a running MCP subprocess, **When** the agent is dropped or cancelled, **Then** the subprocess is terminated within a reasonable timeout.
3. **Given** a running MCP subprocess that crashes, **When** the agent detects the failure, **Then** the MCP tools are marked as unavailable and an event is emitted.

---

### Edge Cases

- What happens when an MCP server returns an error for a tool call? The error is converted to the agent's standard error result and returned to the LLM so it can decide how to proceed.
- What happens when an MCP server takes too long to respond? Tool calls are subject to the same cancellation token as native tools — if cancelled, the pending call is aborted.
- What happens when an MCP server returns content types the agent doesn't support (e.g., binary data)? Unsupported content types are converted to a text description indicating the type was not supported.
- What happens when two MCP servers without prefixes advertise the same tool name? The second registration fails with a clear error at connection time, before the agent loop starts.
- What happens when the MCP server's tool list changes after initial discovery? The initial tool set is used for the session. Dynamic re-discovery is out of scope for this spec.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST connect to MCP servers using stdio transport (subprocess spawning with stdin/stdout communication).
- **FR-002**: System MUST discover available tools from a connected MCP server using the MCP tool listing protocol.
- **FR-003**: System MUST convert MCP tool definitions (name, description, input schema) into the agent's standard tool interface so MCP tools are indistinguishable from native tools to the LLM.
- **FR-004**: System MUST forward tool call requests to the appropriate MCP server and return results to the LLM.
- **FR-005**: System MUST support connecting to multiple MCP servers simultaneously from a single agent.
- **FR-006**: System MUST support optional name prefixing per MCP server to avoid tool name collisions across servers.
- **FR-007**: System MUST route MCP tool calls through the same approval and policy pipeline as native tools.
- **FR-008**: System MUST manage MCP subprocess lifecycle — spawning on agent start and terminating on agent drop or cancellation.
- **FR-009**: System MUST handle MCP server connection failures gracefully, allowing the agent to operate with reduced tool availability.
- **FR-010**: System MUST support tool filtering (allow-list and deny-list) per MCP server connection.
- **FR-011**: System MUST connect to MCP servers using SSE (Server-Sent Events) transport over HTTP.
- **FR-012**: System MUST handle MCP tool call errors by converting them to the agent's standard error result format.
- **FR-013**: System MUST respect the agent's cancellation token when waiting for MCP tool responses.
- **FR-014**: System MUST detect and report tool name collisions across MCP servers at connection time.
- **FR-015**: System MUST be feature-gated so that projects not using MCP incur no compilation or runtime cost.
- **FR-016**: System MUST emit events through the existing agent event system for MCP lifecycle operations: server connect, server disconnect, tool discovery completed, tool call forwarded, and tool call completed.
- **FR-017**: System MUST support optional bearer token authentication for SSE transport connections, integrated with the credential resolver (spec 035).
- **FR-018**: System MUST support optional environment variable overrides when spawning MCP server subprocesses (stdio transport).

### Key Entities

- **MCP Connection**: Represents a configured connection to an MCP server — transport type (stdio or SSE), connection parameters (command + args + optional env vars for stdio; URL + optional bearer token for SSE), optional name prefix, and optional tool filters.
- **MCP Tool**: A tool discovered from an MCP server — wraps the MCP tool definition and implements the agent's standard tool interface, routing execution calls to the originating MCP server.
- **Tool Filter**: A configuration that controls which tools from an MCP server are exposed to the agent — supports allow-list and deny-list patterns.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: An agent with one MCP server configured discovers and registers all advertised tools within 2 seconds of startup.
- **SC-002**: MCP tool calls complete with the same latency characteristics as equivalent native tool calls (overhead from MCP protocol adds less than 50ms per call for stdio transport).
- **SC-003**: An agent connected to 5 MCP servers simultaneously operates without tool name collisions when prefixes are configured.
- **SC-004**: When an MCP server subprocess crashes, the agent detects the failure and updates tool availability within 5 seconds.
- **SC-005**: All existing agent tests pass without modification when the MCP feature is disabled (zero regression).
- **SC-006**: MCP tool calls are subject to the same policy evaluation as native tools — 100% policy coverage, no bypass path.

## Assumptions

- The MCP specification (as of 2025) is stable enough for a non-breaking integration. If the spec changes significantly, the MCP feature gate isolates impact.
- A mature Rust MCP client crate exists (e.g., `rmcp` by the MCP org) that handles JSON-RPC transport, reducing the scope to integration rather than protocol implementation.
- MCP servers follow the standard tool listing and tool call protocol. Non-compliant servers may produce errors that are handled as connection failures.
- Tool re-discovery during a session is not needed. The tool set is fixed at connection time. Hot-reload of MCP tools can be addressed in a future spec.
- MCP prompts and resource protocols are out of scope for this spec. Only the tool protocol is integrated.
