# Feature Specification: TUI: Plan Mode & Approval

**Feature Branch**: `029-tui-plan-mode-approval`
**Created**: 2026-03-20
**Status**: Draft
**Input**: Plan mode (read-only tool restriction, distinct styling, execute transition), tiered approval system (Enabled/Smart/Bypassed, per-tool session trust, classification via requires_approval trait method). References: PRD §16.9, §16.11, HLD TUI, TUI_PHASES T4.

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

1. **Given** Smart mode and an approval prompt, **When** the developer approves with "always approve this tool," **Then** the tool is marked as trusted for this session.
2. **Given** a trusted tool, **When** the agent calls it again, **Then** it executes without an approval prompt.
3. **Given** a trusted tool, **When** the developer revokes trust, **Then** subsequent calls to that tool require approval again.
4. **Given** session trust for a tool, **When** the TUI is restarted, **Then** the trust is not carried over — the tool requires approval again.

---

### User Story 3 - Plan Before Executing with Plan Mode (Priority: P1)

A developer wants the agent to analyze and plan before making changes. They toggle plan mode via Shift+Tab or the /plan command. In plan mode, the agent is restricted to read-only tools — write tools are removed from the agent's available tool set. The UI reflects plan mode with distinct styling (e.g., a different border color or a "PLAN" label). The agent can read files, analyze code, and propose changes without executing them. When the developer is satisfied with the plan, they switch back to execute mode, which re-registers the write tools. Optionally, the plan summary is sent as a follow-up message to guide execution.

**Why this priority**: Plan-then-execute is a critical workflow for complex changes — it lets the developer review the agent's strategy before any modifications occur.

**Independent Test**: Can be tested by toggling plan mode, asking the agent to make a change, verifying only read tools are called, then toggling to execute mode and verifying write tools become available.

**Acceptance Scenarios**:

1. **Given** execute mode is active, **When** the developer presses Shift+Tab or submits /plan, **Then** plan mode activates.
2. **Given** plan mode is active, **When** the agent attempts to use a write tool, **Then** the tool is not available and the agent can only use read-only tools.
3. **Given** plan mode is active, **When** the developer looks at the UI, **Then** a distinct visual indicator (border color or label) shows plan mode is on.
4. **Given** plan mode is active, **When** the developer toggles back to execute mode, **Then** write tools are re-registered and available to the agent.
5. **Given** plan mode produced a plan, **When** switching to execute mode, **Then** the developer is offered the option to send the plan as a follow-up message.

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

- What happens when the developer toggles plan mode while the agent is mid-response with a write tool queued?
- How does the approval prompt behave when multiple tools are called concurrently — are they prompted individually or batched?
- What happens when a tool's approval classification changes during a session (e.g., via dynamic tool registration)?
- How does plan mode interact with tools that are both read and write (e.g., a tool that reads and then modifies)?
- What happens when the developer rejects a tool in Enabled mode that the agent considers essential?
- How does the system handle rapid mode switching (e.g., toggling plan mode multiple times quickly)?
- What happens when all tools are write tools and plan mode is activated — does the agent have zero tools?
- How does per-tool trust interact with Enabled mode (is trust only relevant in Smart mode)?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The TUI MUST support three approval modes: Enabled (prompt for all tools), Smart (auto-approve read-only, prompt for write), and Bypassed (auto-approve all).
- **FR-002**: Smart MUST be the default approval mode.
- **FR-003**: The approval mode MUST be changeable at any time via `#approve on`, `#approve smart`, or `#approve off`.
- **FR-004**: When a tool requires approval, the TUI MUST display the tool name and arguments and wait for the developer to approve or reject.
- **FR-005**: Tool approval classification MUST be determined by the tool's `requires_approval()` trait method.
- **FR-006**: In Smart mode, after approving a tool, the developer MUST be offered the option to trust that tool for the remainder of the session.
- **FR-007**: Per-tool session trust MUST NOT persist across TUI restarts.
- **FR-008**: Plan mode MUST be togglable via Shift+Tab or the /plan command.
- **FR-009**: In plan mode, write tools MUST be removed from the agent's available tool set.
- **FR-010**: Plan mode MUST be visually distinct from execute mode (different border color or label).
- **FR-011**: Switching from plan mode to execute mode MUST re-register write tools.
- **FR-012**: When switching from plan to execute mode, the developer MUST be offered the option to send the plan as a follow-up message.
- **FR-013**: Rejected tool calls MUST inform the agent of the rejection so it can adjust its approach.
- **FR-014**: Per-tool trust MUST be revocable by the developer during the session.

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
