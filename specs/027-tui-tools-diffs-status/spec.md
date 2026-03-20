# Feature Specification: TUI: Tool Panel, Diffs & Status Bar

**Feature Branch**: `027-tui-tools-diffs-status`
**Created**: 2026-03-20
**Status**: Draft
**Input**: Tool panel with spinners and badges, collapsible tool result blocks, inline unified diff view, status bar with model/token/cost/state info, context window progress bar, format helpers. References: PRD §16.6-16.7, §16.10, HLD TUI, TUI_PHASES T3+T4.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Monitor Tool Execution in Real Time (Priority: P1)

A developer watches the agent execute tools. The tool panel appears when tool execution begins, showing each active tool with an animated spinner. As tools complete, the spinner is replaced by a check badge (success) or cross badge (failure). After all tools finish and a timeout elapses, the tool panel fades away to reclaim screen space. The developer can see at a glance which tools are running, which succeeded, and which failed.

**Why this priority**: Tool execution visibility is critical — without it, the developer cannot tell what the agent is doing or whether something went wrong.

**Independent Test**: Can be tested by triggering an agent action that calls tools, verifying spinners appear for active tools and badges appear on completion.

**Acceptance Scenarios**:

1. **Given** the agent begins a tool call, **When** the tool panel updates, **Then** the tool appears with an animated spinner.
2. **Given** a tool completes successfully, **When** the panel updates, **Then** the spinner is replaced by a check badge.
3. **Given** a tool fails, **When** the panel updates, **Then** the spinner is replaced by a cross badge.
4. **Given** all tools have completed, **When** a timeout period elapses, **Then** the tool panel auto-hides.
5. **Given** no tools are active, **When** the developer looks at the UI, **Then** the tool panel is not visible.

---

### User Story 2 - Review File Changes as Inline Diffs (Priority: P1)

A developer reviews file modifications made by the agent. When a tool produces file changes, the conversation displays an inline unified diff. Added lines are shown in green, removed lines in red, and context lines are dimmed. New files are displayed as all-addition diffs. Large diffs are truncated with an indication of how many lines were omitted. The diff is syntax-highlighted to match the file type when possible.

**Why this priority**: Reviewing changes before they are applied is essential for developer trust and safety — diffs are the standard way developers verify modifications.

**Independent Test**: Can be tested by having the agent modify a file, verifying the diff shows additions in green and removals in red with correct content.

**Acceptance Scenarios**:

1. **Given** a tool modifies an existing file, **When** the result is displayed, **Then** an inline unified diff shows additions in green and removals in red.
2. **Given** a tool creates a new file, **When** the result is displayed, **Then** all lines are shown as additions.
3. **Given** unchanged lines surround a change, **When** the diff is displayed, **Then** context lines appear in a dimmed style.
4. **Given** a diff exceeds a size threshold, **When** displayed, **Then** the diff is truncated with a summary of omitted lines.
5. **Given** a diff for a file with a known type, **When** displayed, **Then** the diff content is syntax-highlighted.

---

### User Story 3 - Track Resource Consumption via Status Bar (Priority: P1)

A developer monitors the agent's resource usage and state through a persistent status bar. The status bar displays: the current model name, token usage (formatted as human-readable counts like "12.5K"), estimated cost, the agent's state (idle, running, or error), a retry indicator when retries are in progress, and elapsed time for the current operation. This information is always visible at a glance.

**Why this priority**: Resource awareness prevents surprise costs and helps developers understand agent behavior — this is always-visible, non-intrusive information.

**Independent Test**: Can be tested by running an agent interaction and verifying the status bar updates to show the correct model, token count, cost, and state transitions.

**Acceptance Scenarios**:

1. **Given** the agent is idle, **When** the developer looks at the status bar, **Then** the state shows "idle" and elapsed time is not shown.
2. **Given** the agent is generating a response, **When** the status bar updates, **Then** the state shows "running" and elapsed time increments.
3. **Given** the agent encounters an error, **When** the status bar updates, **Then** the state shows "error."
4. **Given** tokens have been consumed, **When** the status bar updates, **Then** token usage is displayed in human-readable format (e.g., "1.2K", "3.5M").
5. **Given** a retry is in progress, **When** the status bar updates, **Then** a retry indicator is visible.
6. **Given** an active session, **When** the developer looks at the status bar, **Then** the model name and estimated cost are displayed.

---

### User Story 4 - View Context Window Utilization (Priority: P2)

A developer monitors how much of the model's context window has been consumed. A compact progress bar (10 characters wide) in the status bar shows context utilization. The bar color changes to indicate urgency: green when under 60% full, yellow between 60-85%, and red above 85%. This gives the developer advance warning that context compaction or a new conversation may be needed soon.

**Why this priority**: Context awareness prevents mid-conversation failures, but is secondary to core tool monitoring and status information.

**Independent Test**: Can be tested by running a conversation that gradually fills the context, verifying the gauge color transitions from green to yellow to red.

**Acceptance Scenarios**:

1. **Given** context usage is below 60%, **When** the gauge renders, **Then** it is green.
2. **Given** context usage is between 60% and 85%, **When** the gauge renders, **Then** it is yellow.
3. **Given** context usage exceeds 85%, **When** the gauge renders, **Then** it is red.
4. **Given** context usage changes, **When** the status bar updates, **Then** the gauge reflects the current percentage.

---

### User Story 5 - Expand and Collapse Tool Result Blocks (Priority: P2)

A developer manages screen space by collapsing and expanding tool result blocks in the conversation. When a tool result first appears, it is expanded to show full details. After a timeout, it auto-collapses to a single-line summary to reduce clutter. The developer can toggle expansion with a keyboard shortcut (F2) and cycle selection between tool blocks with Shift+arrow keys. If the developer has manually expanded a block, it resists auto-collapse — the developer's explicit choice is preserved.

**Why this priority**: Collapsible blocks reduce visual clutter in tool-heavy conversations, but are a usability enhancement rather than core functionality.

**Independent Test**: Can be tested by triggering multiple tool calls, waiting for auto-collapse, then pressing F2 to expand a selected block and verifying it stays expanded.

**Acceptance Scenarios**:

1. **Given** a tool result appears, **When** initially displayed, **Then** it is expanded showing full details.
2. **Given** an expanded tool result, **When** a timeout elapses, **Then** the block auto-collapses to a one-line summary.
3. **Given** a collapsed tool block is selected, **When** F2 is pressed, **Then** the block expands.
4. **Given** the developer has manually expanded a block, **When** the auto-collapse timeout elapses, **Then** the block remains expanded.
5. **Given** multiple tool blocks, **When** Shift+Up/Down is pressed, **Then** selection cycles between tool blocks.

---

### Edge Cases

- What happens when dozens of tools execute concurrently — does the tool panel scale or scroll?
- How does the diff view handle binary files or files with no newline at end?
- What happens when the model name is very long — does the status bar truncate it?
- How does the context gauge behave when the model does not report context usage?
- What happens when a tool result is extremely large (megabytes of output)?
- How does the diff computation handle files that are entirely replaced (no common lines)?
- What happens when cost information is not available for the current model?
- How does the tool panel behave when a tool hangs indefinitely (no completion event)?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The tool panel MUST display an animated spinner for each actively executing tool.
- **FR-002**: The tool panel MUST replace the spinner with a check badge on success or a cross badge on failure.
- **FR-003**: The tool panel MUST auto-hide after a configurable timeout when all tools have completed.
- **FR-004**: The tool panel MUST appear when tools begin executing and hide when idle.
- **FR-005**: Tool result blocks MUST default to expanded and auto-collapse to a one-line summary after a timeout.
- **FR-006**: Tool result blocks MUST be toggleable via F2 keyboard shortcut.
- **FR-007**: Tool result blocks MUST support selection cycling via Shift+Up/Shift+Down.
- **FR-008**: User-expanded tool result blocks MUST resist auto-collapse.
- **FR-009**: Inline diffs MUST use unified diff format with additions in green, removals in red, and context lines dimmed.
- **FR-010**: New files MUST be displayed as all-addition diffs.
- **FR-011**: Large diffs MUST be truncated with a summary of omitted lines.
- **FR-012**: Diffs MUST be syntax-highlighted when the file type is recognized.
- **FR-013**: The status bar MUST display model name, token usage, estimated cost, agent state, and elapsed time.
- **FR-014**: The status bar MUST show a retry indicator during retry operations.
- **FR-015**: Token counts MUST be formatted in human-readable notation (K for thousands, M for millions).
- **FR-016**: The context window gauge MUST be 10 characters wide and color-coded: green (<60%), yellow (60-85%), red (>85%).
- **FR-017**: The diff computation MUST use a longest-common-subsequence algorithm.

### Key Entities

- **ToolPanel**: The UI component that shows active and recently completed tool executions with spinners and badges. Auto-hides when idle.
- **ToolResultBlock**: A collapsible section in the conversation displaying a tool's output. Has expanded and collapsed states, with auto-collapse behavior and user override.
- **DiffView**: A visual representation of file changes in unified diff format, with syntax highlighting and color-coded additions/removals.
- **StatusBar**: A persistent UI element displaying model information, token usage, cost, agent state, retry status, and elapsed time.
- **ContextGauge**: A compact progress bar within the status bar showing context window utilization with color-coded urgency thresholds.
- **FormatHelper**: Utility functions for rendering human-readable token counts, elapsed time, and context gauge percentages.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Active tool execution is visible within one render frame of the tool call starting.
- **SC-002**: Tool completion status (success/failure) is distinguishable at a glance via badges.
- **SC-003**: File changes are displayed as diffs with correct additions, removals, and context lines.
- **SC-004**: The developer can determine the agent's current state, token usage, and cost without navigating away from the conversation.
- **SC-005**: Context window utilization color transitions occur at the documented thresholds (60%, 85%).
- **SC-006**: Auto-collapsed tool blocks reduce visual clutter while remaining expandable on demand.

## Assumptions

- The TUI scaffold, event loop, and conversation view from specs 025-026 are in place.
- Tool execution events (start, progress, completion, failure) are emitted by the agent event system.
- File content before and after modification is available to compute diffs (provided by the tool result or agent context).
- Token usage and cost data are provided by the agent or adapter layer; the TUI only displays them.
- The context window size (maximum tokens) for the current model is known so that utilization percentage can be calculated.
- The auto-hide and auto-collapse timeouts are configurable via the TUI config file.
