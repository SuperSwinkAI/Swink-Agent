# Feature Specification: TransferToAgent Tool & Handoff Safety

**Feature Branch**: `040-agent-transfer-handoff`  
**Created**: 2026-04-02  
**Status**: Draft  
**Input**: User description: "TransferToAgent Tool & Handoff Safety — generic TransferToAgent tool and circular transfer detection for agent handoff primitives"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Agent Transfers Conversation to Another Agent (Priority: P1)

A library consumer building a customer support system has specialized agents — a triage agent, a billing agent, and a technical support agent. When the triage agent determines a user's issue is billing-related, it calls the `transfer_to_agent` tool to hand off the conversation to the billing agent. The triage agent's turn ends, the transfer signal surfaces to the orchestrating code, and the consumer dispatches the billing agent to continue the conversation with full context.

**Why this priority**: Transfer is the core capability. Without it, agents cannot delegate to other agents mid-conversation — they can only invoke sub-agents (which return results back, not hand off). Transfer enables true multi-agent routing workflows.

**Independent Test**: Can be fully tested by configuring an agent with the transfer tool, having the LLM call it with a target agent name and reason, and verifying the agent loop terminates with a transfer signal containing the target, reason, and conversation history.

**Acceptance Scenarios**:

1. **Given** an agent configured with `TransferToAgentTool` and a registered agent "billing", **When** the LLM calls `transfer_to_agent` with `agent_name: "billing"` and `reason: "billing issue"`, **Then** the agent loop terminates and the result carries a transfer signal with target "billing", reason "billing issue", and the current conversation history.
2. **Given** an agent configured with `TransferToAgentTool`, **When** the LLM calls `transfer_to_agent` with a target agent that does not exist in the registry, **Then** the tool returns an error result indicating the target agent was not found and the agent loop continues (the LLM can retry or respond to the user).
3. **Given** an agent configured with `TransferToAgentTool` and a context summary, **When** the LLM provides a `context_summary` parameter, **Then** the transfer signal includes the summary so the target agent can receive a concise handoff brief.

---

### User Story 2 - Consumer Restricts Which Agents Can Be Transfer Targets (Priority: P1)

A library consumer wants to restrict which agents a given agent can transfer to. The triage agent should only be able to transfer to "billing" or "technical-support" — not to "admin" or "internal-ops". The consumer configures the transfer tool with an allowed targets list. Transfer attempts to unlisted agents are rejected with an error.

**Why this priority**: Unrestricted transfers in production systems are a safety risk — an LLM could hallucinate agent names or transfer to agents that shouldn't receive certain conversations. Allowed targets is the access control mechanism.

**Independent Test**: Can be fully tested by creating a transfer tool with allowed targets ["billing", "technical"], attempting to transfer to "billing" (succeeds) and "admin" (fails with error).

**Acceptance Scenarios**:

1. **Given** a transfer tool configured with allowed targets ["billing", "technical-support"], **When** the LLM transfers to "billing", **Then** the transfer proceeds normally.
2. **Given** a transfer tool configured with allowed targets ["billing", "technical-support"], **When** the LLM transfers to "admin", **Then** the tool returns an error result indicating "admin" is not an allowed target.
3. **Given** a transfer tool configured with no allowed targets restriction (unrestricted), **When** the LLM transfers to any registered agent, **Then** the transfer proceeds as long as the target exists in the registry.

---

### User Story 3 - System Detects and Prevents Circular Transfers (Priority: P1)

A library consumer running a multi-agent routing system needs protection against infinite handoff loops. Agent A transfers to Agent B, which transfers back to Agent A, creating an infinite cycle. The transfer chain tracking mechanism detects the circular reference and rejects the transfer with a clear error before the cycle begins.

**Why this priority**: Without circular detection, a single misdirected transfer can create an infinite loop that consumes resources until the system is killed. This is a safety-critical feature for any production multi-agent deployment.

**Independent Test**: Can be fully tested by creating a transfer chain, pushing agents A then B, and verifying that pushing A again returns a circular transfer error.

**Acceptance Scenarios**:

1. **Given** a transfer chain with agents [A, B], **When** a transfer to agent A is attempted, **Then** the chain rejects it with a circular transfer error identifying agent A as the duplicate.
2. **Given** a transfer chain with depth limit 5, **When** a 6th unique agent transfer is attempted, **Then** the chain rejects it with a max depth exceeded error.
3. **Given** a fresh transfer chain (new user message), **When** agent A is pushed, **Then** it succeeds regardless of what happened in previous chains (chains are per-message, not per-session).
4. **Given** a transfer chain with agents [A, B, C], **When** a transfer to agent D is attempted, **Then** it succeeds (D is not in the chain).

---

### User Story 4 - Consumer Observes Transfer Activity via Events (Priority: P2)

A library consumer building a monitoring dashboard wants to track transfer activity — when transfers are requested, completed, rejected, or detected as circular. Transfer events are emitted through the existing event system so the consumer's existing event listeners receive them automatically.

**Why this priority**: Observability is important for production debugging and audit trails but is not required for transfers to function. It extends the existing event infrastructure.

**Independent Test**: Can be fully tested by registering an event listener, triggering a transfer, and verifying the listener received the appropriate transfer event(s).

**Acceptance Scenarios**:

1. **Given** an event listener on the agent, **When** a transfer is requested, **Then** the listener receives a transfer-requested event with the source agent, target agent, and reason.
2. **Given** an event listener on the agent, **When** a transfer is rejected (target not found or not allowed), **Then** the listener receives a transfer-rejected event with the rejection reason.
3. **Given** an event listener on the orchestrating code, **When** a circular transfer is detected, **Then** the listener receives a circular-transfer-detected event with the full chain.

---

### User Story 5 - Consumer Orchestrates Transfer Execution (Priority: P2)

A library consumer receives a transfer signal from the agent loop and must decide how to execute the handoff. The transfer signal contains the target agent name, reason, optional context summary, and conversation history. The consumer uses this information to dispatch the target agent — they control how much history to include, whether to add a system prompt about the handoff context, and how to handle the target agent's response.

**Why this priority**: The library intentionally does not execute transfers — it signals them. This story validates that the signal carries enough information for consumers to build their own orchestration logic on top.

**Independent Test**: Can be fully tested by triggering a transfer, inspecting the transfer signal in the agent result, and verifying it contains all required fields (target, reason, context summary, conversation history).

**Acceptance Scenarios**:

1. **Given** an agent that returns a transfer signal, **When** the consumer inspects the result, **Then** the result's stop reason is Transfer and contains the target agent name, reason string, and conversation history.
2. **Given** a transfer signal with a context summary, **When** the consumer inspects it, **Then** the context summary is available as an optional field.
3. **Given** a transfer signal, **When** the consumer reads the conversation history, **Then** it contains all messages from the current agent's session so the target agent can continue with context.

---

### Edge Cases

- What happens when the LLM calls `transfer_to_agent` alongside other tool calls in the same turn? The transfer tool's result carries the transfer signal. The agent loop processes all tool results, detects the transfer signal among them, and terminates the turn with the transfer stop reason. Other tool results from the same turn are included in the conversation history that transfers to the target agent.
- What happens when the LLM calls `transfer_to_agent` multiple times in one turn? Only the first transfer signal is honored. Subsequent transfer tool calls in the same turn return an error result ("transfer already pending").
- What happens when `allowed_targets` is an empty set? No transfers are possible — every transfer attempt returns an error. This is a valid (if unusual) configuration that effectively disables the tool.
- What happens when an agent transfers to itself? The transfer chain detects this as a circular transfer (the current agent is always the first entry in the chain) and rejects it.
- What happens when the agent loop is cancelled while a transfer is in progress? The cancellation takes precedence. The loop terminates with an Aborted stop reason, not a Transfer.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST provide a `TransferToAgentTool` that implements the agent tool trait and signals the agent loop to transfer conversation to a named target agent. On successful transfer, the tool result text MUST be a brief confirmation message (e.g., "Transfer to {agent_name} initiated.") so the LLM sees a clear outcome.
- **FR-002**: The tool MUST accept three parameters: `agent_name` (required string), `reason` (required string), and `context_summary` (optional string).
- **FR-003**: The tool MUST validate that the target agent exists in the agent registry at execution time. If the target does not exist, the tool MUST return an error result and the agent loop MUST continue.
- **FR-004**: The tool MUST support an optional allowed targets restriction. When configured, transfer attempts to agents not in the allowed set MUST be rejected with an error result.
- **FR-005**: Allowed targets MUST be validated at execution time, not construction time, so agents can be registered in the registry after the tool is created.
- **FR-006**: System MUST provide a `TransferSignal` carrying the target agent name, reason, optional context summary, and conversation history.
- **FR-007**: The agent loop MUST surface the transfer signal via the stop reason mechanism so the caller receives it as part of the normal result — no new return types.
- **FR-008**: The tool result type MUST support an optional transfer signal field. When this field is present on any tool result, the agent loop MUST terminate the current turn and return the transfer signal to the caller via the stop reason. The loop itself MUST NOT execute the transfer.
- **FR-009**: If the LLM calls the transfer tool alongside other tools in the same turn, all tool results MUST be processed. The transfer signal terminates the turn after processing.
- **FR-010**: If the LLM calls the transfer tool multiple times in one turn, only the first transfer MUST be honored. Subsequent calls MUST return an error result.
- **FR-011**: System MUST provide a `TransferChain` that tracks an ordered sequence of agent names and a configurable maximum depth (default: 5).
- **FR-012**: `TransferChain` MUST reject transfers that would create a circular reference — if the target agent name already appears anywhere in the chain.
- **FR-013**: `TransferChain` MUST reject transfers that would exceed the configured maximum depth.
- **FR-014**: Transfer chains MUST be scoped per top-level user message. A new user message resets the chain.
- **FR-015**: System MUST emit events for transfer activity: transfer requested, transfer rejected, and circular transfer detected.
- **FR-016**: The transfer tool MUST be a standard tool with no special loop hooks — the loop recognizes the transfer signal in tool results, not via a custom mechanism.
- **FR-017**: The conversation history included in the transfer signal MUST contain all messages from the current agent's session so the target agent can continue with full context. The tool returns a partial signal (target, reason, summary only); the agent loop MUST enrich it with the conversation history before surfacing it via the stop reason.

### Key Entities

- **TransferToAgentTool**: The tool implementation that LLMs call to request a handoff. Holds a reference to the agent registry and an optional allowed targets set. The mechanism through which agents express intent to transfer.
- **TransferSignal**: Data structure carrying all information needed for the target agent to continue the conversation — target name, reason, optional context summary, and conversation history. The handoff payload.
- **TransferChain**: Safety mechanism that tracks the sequence of agents involved in a transfer chain and enforces depth limits and circular reference detection. The guard against infinite handoff loops.
- **TransferError**: Error type with variants for circular transfer (agent already in chain) and max depth exceeded. The rejection signal when a transfer would be unsafe.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Library consumers can configure an agent with a transfer tool and receive a transfer signal when the LLM decides to hand off, with zero custom glue code for the signaling path.
- **SC-002**: Transfer attempts to nonexistent or disallowed agents are rejected immediately with clear error messages — the LLM can self-correct or inform the user.
- **SC-003**: Circular transfer chains are detected and rejected before executing — no infinite handoff loops are possible when the chain is used.
- **SC-004**: Transfer signals carry sufficient context (target, reason, summary, history) for consumers to build any orchestration pattern on top — triage routing, escalation, round-robin, etc.
- **SC-005**: The transfer mechanism adds zero overhead to agents that do not use it — no new fields, no new checks in the loop for agents without the transfer tool.
- **SC-006**: Transfer integrates with existing tool approval mechanisms — if tool approval is enabled, the transfer tool is subject to the same approval flow as any other tool.

## Assumptions

- The agent loop does not execute transfers. It returns a transfer signal via the stop reason and the caller (orchestrator, pipeline executor, or consumer code) decides what to do. This keeps the loop simple and the orchestration concern external.
- `TransferToAgentTool` lives in the core crate because it depends on core types (`AgentTool`, `AgentRegistry`, `StopReason`). It's a library primitive, not an application-level concern. It is feature-gated under a `transfer` flag (default-enabled), following the `builtin-tools` pattern.
- The transfer signal includes the full conversation history from the current agent's session. The consumer decides how much of this history to forward to the target agent — they may truncate, summarize, or pass it all.
- `TransferChain` is a separate struct from the tool itself. It is owned by the orchestrating code, not by the tool. The tool returns a signal; the orchestrator consults the chain before acting on it. The chain is passed as an argument to the orchestrator/executor run method — the orchestrator creates a new chain per user message and carries it forward through transfers.
- The stop reason mechanism is extended with a Transfer variant. This is an additive, backward-compatible change — existing code that matches on stop reasons will hit a wildcard arm or a `non_exhaustive` guard.
- Transfer events use the existing event system. No new event infrastructure is needed.
- An agent transferring to itself is always a circular transfer (the agent is always the first entry in the chain). This is detected and rejected by the chain, not by the tool.
- When multiple tools are called in the same turn and one of them is a transfer, the transfer takes effect after all tool results are processed. The tool results (including non-transfer tools) are part of the conversation history that transfers to the target agent.
- The transfer signal is encoded as an optional field on the tool result type (not a sentinel string or special error). Normal tool results have this field as None. Transfer results set it to the transfer signal. This is type-safe and backward-compatible — existing tool results are unaffected.

## Clarifications

### Session 2026-04-02

- Q: How is the transfer signal encoded in the tool result? → A: Optional `TransferSignal` field on the tool result type — None for normal results, Some for transfers. Type-safe, backward-compatible.
- Q: Who populates conversation history in TransferSignal? → A: Tool returns partial signal (target, reason, summary); loop enriches it with conversation history.
- Q: How is TransferChain threaded through transfers? → A: Passed as argument to the orchestrator/executor run method. Orchestrator creates per user message and carries forward.
- Q: Should TransferToAgentTool be behind a feature gate? → A: Yes, feature-gated under `transfer` (default-enabled), following the `builtin-tools` pattern.
- Q: What text does the tool result show to the LLM on successful transfer? → A: Brief confirmation: "Transfer to {agent_name} initiated."
