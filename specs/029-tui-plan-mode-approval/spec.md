# Feature Specification: TUI: Plan Mode & Approval

**Feature Branch**: `029-tui-plan-mode-approval`
**Created**: 2026-03-20
**Status**: Draft
**Input**: Plan mode (read-only tool restriction, distinct styling, execute transition), tiered approval system (Enabled/Smart/Bypassed, per-tool session trust, classification via requires_approval trait method). References: PRD §16.9, §16.11, HLD TUI, TUI_PHASES T4.

## Clarifications

### Session 2026-03-22

- Q: Should plan mode toggle be guarded while the agent is streaming? → A: Yes — ignore toggle while streaming, apply only when idle.
- Q: How should the "always approve this tool" offer be presented? → A: Brief inline follow-up prompt after approval, auto-dismisses after 3 seconds.
- Q: Should there be an explicit latency target for approval resolution? → A: No — resolution is synchronous via oneshot channel, effectively instant (<1 frame).
- Q: How should per-tool trust be revoked? → A: `#approve untrust <tool_name>` for specific tool, `#approve untrust` to clear all.
- Q: What constitutes "the plan" when sending as user message? → A: All assistant messages from the plan mode session, concatenated.
- Q: What if no assistant messages during plan mode when approving? → Deferred — low impact, handle in planning.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Control Which Tools Require Approval (Priority: P1)

A developer wants to control the level of oversight they have over tool execution. The TUI provides three approval modes: Enabled (prompt for every tool call), Smart (auto-approve read-only tools, prompt for write tools), and Bypassed (auto-approve everything). Smart mode is the default. The developer can switch modes at any time using the `#approve` command with `on` (Enabled), `smart`, or `off` (Bypassed). When a tool requires approval, the TUI pauses execution and presents the tool name, arguments, and approve/reject options. The developer can approve or reject the call.

**Why this priority**: Approval is a safety-critical feature — it prevents the agent from making unwanted modifications. Without it, developers cannot safely use write tools.

**Independent Test**: Can be tested by triggering a write tool in Smart mode and verifying an approval prompt appears, then switching to Bypassed mode and verifying the same tool auto-executes.

**Acceptance Scenarios**:

1. **Given** Smart mode is active, **When** the agent calls a read-only tool, **Then** the tool executes without prompting.
2. **Given** Smart mode is active, **When** the agent calls a write tool, **Then** an approval prompt is presented with the tool name and arguments.
3. **Given** an approval prompt, **When** the developer approves, **Then** the tool executes.
4. **Given** an approval prompt, **When** the developer rejects, **Then** the tool is not executed and the agent is informed of the rejection.
5. **Given** any mode, **When** `#approve smart` is submitted, **Then** the mode switches to Smart.
6. **Given** any mode, **When** `#approve on` is submitted, **Then** the mode switches to Enabled (prompt for all).
7. **Given** any mode, **When** `#approve off` is submitted, **Then** the mode switches to Bypassed (no prompts).

---

### User Story 2 - Grant Per-Tool Session Trust (Priority: P1)

A developer is working in Smart mode and gets prompted to approve a write tool. After approving, the developer is given the option to "always approve this tool for this session." If they accept, subsequent calls to that same tool are auto-approved for the remainder of the session without further prompting. This trust is per-session only and does not persist across restarts. The developer can see which tools have been trusted and can revoke trust at any time.

**Why this priority**: Repeated approval prompts for the same tool disrupt flow. Per-tool trust balances safety with productivity in Smart mode.

**Independent Test**: Can be tested by approving a tool with "always approve," triggering the same tool again, and verifying no prompt appears.

**Acceptance Scenarios**:

1. **Given** Smart mode and an approval prompt, **When** the developer approves, **Then** a brief inline follow-up prompt ("Always approve this tool? y/n") appears and auto-dismisses after 3 seconds. If accepted, the tool is marked as trusted for this session.
2. **Given** a trusted tool, **When** the agent calls it again, **Then** it executes without an approval prompt.
3. **Given** a trusted tool, **When** the developer runs `#approve untrust <tool_name>`, **Then** subsequent calls to that tool require approval again.
4. **Given** session trust for a tool, **When** the TUI is restarted, **Then** the trust is not carried over — the tool requires approval again.

---

### User Story 3 - Plan Before Executing with Plan Mode (Priority: P1)

A developer wants the agent to analyze and plan before making changes. They enter plan mode via Shift+Tab or the /plan command. In plan mode, the agent is restricted to read-only tools — write tools are removed from the agent's available tool set. The UI reflects plan mode with distinct styling (e.g., a different border color or a "PLAN" label). The agent can read files, analyze code, and propose changes without executing them. When the developer is satisfied with the plan, they press Shift+Tab or /plan again, which presents an "Approve plan?" prompt — styled like a tool approval prompt. On approval, the TUI exits plan mode, re-registers write tools, and automatically sends all plan-mode assistant messages as the next user message in normal execute mode. The agent then executes against the plan without additional developer input.

**Why this priority**: Plan-then-execute is a critical workflow for complex changes — it lets the developer review the agent's strategy before any modifications occur.

**Independent Test**: Can be tested by toggling plan mode, asking the agent to make a change, verifying only read tools are called, then approving the plan and verifying write tools become available and the plan is sent as the next user message.

**Acceptance Scenarios**:

1. **Given** execute mode is active, **When** the developer presses Shift+Tab or submits /plan, **Then** plan mode activates.
2. **Given** plan mode is active, **When** the agent attempts to use a write tool, **Then** the tool is not available and the agent can only use read-only tools.
3. **Given** plan mode is active, **When** the developer looks at the UI, **Then** a `PLAN` badge appears in the status bar and assistant messages are labeled "Plan" in blue.
4. **Given** plan mode is active, **When** the developer presses Shift+Tab or submits /plan, **Then** an "Approve plan?" prompt appears, styled like a tool approval prompt.
5. **Given** an "Approve plan?" prompt, **When** the developer approves, **Then** plan mode deactivates, write tools are re-registered, and all plan-mode assistant messages are automatically sent as the next user message in execute mode.
6. **Given** an "Approve plan?" prompt, **When** the developer rejects, **Then** plan mode remains active and the developer can continue refining the plan.

---

### User Story 4 - Classify Tools for Approval Decisions (Priority: P2)

The approval system needs to determine whether a tool requires approval. Each tool declares whether it requires approval through a trait method. Read-only tools (e.g., file reading, search) declare themselves as not requiring approval. Write tools (e.g., file writing, command execution) declare themselves as requiring approval. The classification is intrinsic to the tool definition, not maintained as a separate list. In Smart mode, this classification drives the auto-approve/prompt decision.

**Why this priority**: Correct classification is foundational to Smart mode but is a system-level concern rather than a direct user interaction.

**Independent Test**: Can be tested by registering tools with different approval classifications, running in Smart mode, and verifying each tool is correctly auto-approved or prompted.

**Acceptance Scenarios**:

1. **Given** a tool that declares `requires_approval = false`, **When** called in Smart mode, **Then** it auto-executes.
2. **Given** a tool that declares `requires_approval = true`, **When** called in Smart mode, **Then** an approval prompt is shown.
3. **Given** Enabled mode, **When** any tool is called regardless of classification, **Then** an approval prompt is shown.
4. **Given** Bypassed mode, **When** any tool is called regardless of classification, **Then** it auto-executes.

---

### Edge Cases

- **Mid-response plan toggle**: When the developer toggles plan mode while the agent is mid-response, write tools are removed from the tool set immediately. Any queued write tool call that has not yet entered approval will fail lookup. Already-dispatched calls complete normally.
- **Concurrent tool approval**: Approval prompts are presented **sequentially, one at a time**. Tool dispatch loops through each tool call in order during pre-processing; the TUI holds a single `pending_approval` slot. Phase 3 (execution) runs approved tools concurrently, but approval itself is serial.
- **Dynamic tool re-registration**: If a tool's `requires_approval()` classification changes mid-session (e.g., via dynamic registration), the new classification takes effect immediately on the next tool dispatch. No additional safeguards or notifications are provided.
- **Mixed read/write tools**: Tool classification is **binary by design** — `requires_approval()` returns a `bool`. A tool that performs both reads and writes must declare `requires_approval() = true` (write). Plan mode removes all `requires_approval() == true` tools; there is no hybrid category.
- **Rejection of essential tool**: Rejected tool calls produce an error tool result (`is_error: true`) sent back to the agent. The agent must adapt its approach (e.g., choose alternative tools or inform the user). No special handling for "essential" tools.
- **Rapid mode switching**: Plan mode toggle is **ignored while the agent is actively streaming a response**. The toggle only takes effect when the agent is idle. This prevents mid-turn tool set changes that could confuse the agent. Each toggle still performs an immediate save/restore — no debounce beyond the streaming guard.
- **All tools are write tools**: If all registered tools have `requires_approval() == true`, activating plan mode results in an **empty tool set**. The agent continues but cannot call any tools until the developer switches back to execute mode.
- **Per-tool trust in Enabled mode**: Session trust is **only checked in Smart mode**. In Enabled mode, all tool calls go through the approval callback regardless of trust status. The trust set may be populated but is ignored outside Smart mode.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The TUI MUST support three approval modes: Enabled (prompt for all tools), Smart (auto-approve read-only, prompt for write), and Bypassed (auto-approve all).
- **FR-002**: Smart MUST be the default approval mode.
- **FR-003**: The approval mode MUST be changeable at any time via `#approve on`, `#approve smart`, or `#approve off`.
- **FR-004**: When a tool requires approval, the TUI MUST display an inline prompt in the tool panel showing the tool name, a summary of arguments (sensitive values redacted), and `Approve? [Y/n]`. The developer approves with `y` (or Enter) and rejects with `n`. Resolved decisions are shown briefly (✓ Approved / ✗ Rejected) and auto-pruned after 2 seconds.
- **FR-005**: Tool approval classification MUST be determined by the tool's `requires_approval()` trait method.
- **FR-006**: In Smart mode, after approving a tool, the TUI MUST display a brief inline follow-up prompt ("Always approve this tool? y/n") that auto-dismisses after 3 seconds if not answered. Accepting marks the tool as trusted for the session.
- **FR-007**: Per-tool session trust MUST NOT persist across TUI restarts.
- **FR-008**: Plan mode MUST be togglable via Shift+Tab or the /plan command.
- **FR-009**: In plan mode, write tools MUST be removed from the agent's available tool set.
- **FR-010**: Plan mode MUST be visually distinct from execute mode: a `PLAN` badge in the status bar (blue accent via `plan_color()`), and assistant messages sent during plan mode labeled "Plan" instead of "Assistant" with the same blue accent.
- **FR-011**: Exiting plan mode MUST present an "Approve plan?" prompt styled like a tool approval prompt. On approval, write tools are re-registered; on rejection, plan mode remains active.
- **FR-012**: When the plan is approved, all assistant messages from the plan mode session MUST be automatically sent as the next user message in execute mode. This is not optional — the plan is always sent on approval.
- **FR-013**: Rejected tool calls MUST inform the agent of the rejection so it can adjust its approach.
- **FR-014**: Per-tool trust MUST be revocable via `#approve untrust <tool_name>` (specific tool) or `#approve untrust` (clear all trusted tools).
- **FR-015**: Plan mode toggle MUST be ignored while the agent is actively streaming a response. The toggle takes effect only when the agent is idle.

### Key Entities

- **ApprovalMode**: The current approval policy — one of Enabled, Smart, or Bypassed. Determines which tool calls require explicit developer approval.
- **ApprovalPrompt**: The UI element presented when a tool call requires approval, showing the tool name, arguments, and approve/reject actions.
- **SessionTrust**: A per-session set of tools that the developer has marked as "always approve." Only relevant in Smart mode. Cleared on restart.
- **PlanMode**: A state toggle that restricts the agent to read-only tools and applies distinct visual styling. Toggled via keyboard shortcut or command.
- **ToolClassification**: The approval property declared by each tool via its trait method, indicating whether it requires approval (read-only vs. write).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: In Smart mode, read-only tools execute without prompting and write tools trigger an approval prompt.
- **SC-002**: Per-tool session trust eliminates repeated prompts for the same tool within a session.
- **SC-003**: Plan mode prevents all write tool execution — zero write tools are available to the agent.
- **SC-004**: Mode switching (plan/execute, approval modes) takes effect within one render frame.
- **SC-005**: The current approval mode and plan/execute state are always visually indicated in the UI.
- **SC-006**: A developer can go from plan to execute and have the agent carry out the planned changes.

## Assumptions

- The TUI scaffold, event loop, input editor, conversation view, and command system from specs 025-028 are in place.
- The tool system's trait includes a `requires_approval()` method that tools implement to declare their classification.
- The agent supports dynamic tool registration and deregistration (adding/removing tools from the active set at runtime).
- Plan mode and approval mode are independent — plan mode restricts available tools, approval mode controls prompting for the tools that are available.
- The agent can accept rejection signals and adjust its behavior (e.g., choose alternative tools or inform the user).
- The "PLAN" label or distinct styling is part of the color theme and respects user theme customization from spec 025.
