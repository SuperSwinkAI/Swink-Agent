# Feature Specification: TUI: Tool Panel, Diffs & Status Bar

**Feature Branch**: `027-tui-tools-diffs-status`
**Created**: 2026-03-20
**Status**: Complete
**Input**: Tool panel with spinners and badges, collapsible tool result blocks, inline unified diff view, status bar with model/token/cost/state info, context window progress bar, format helpers. References: PRD §16.6-16.7, §16.10, HLD TUI, TUI_PHASES T3+T4.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Monitor Tool Execution in Real Time (Priority: P1)

A developer watches the agent execute tools. The tool panel appears when tool execution begins, showing each active tool with an animated spinner. As tools complete, the spinner is replaced by a check badge (success) or cross badge (failure). After all tools finish and a timeout elapses, the tool panel fades away to reclaim screen space. The developer can see at a glance which tools are running, which succeeded, and which failed. **[Addition]** While a tool is still running, the panel also shows a truncated preview of its most recent streamed output line next to the spinner, giving the developer live insight into long-running tools (e.g. shell commands) before they complete.

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

A developer reviews file modifications made by the agent. When a tool produces file changes, the conversation displays an inline unified diff. Added lines are shown in green, removed lines in red, and context lines are dimmed. New files are displayed as all-addition diffs. Large diffs are truncated with an indication of how many lines were omitted.

**Why this priority**: Reviewing changes before they are applied is essential for developer trust and safety — diffs are the standard way developers verify modifications.

**Independent Test**: Can be tested by having the agent modify a file, verifying the diff shows additions in green and removals in red with correct content.

**Acceptance Scenarios**:

1. **Given** a tool modifies an existing file, **When** the result is displayed, **Then** an inline unified diff shows additions in green and removals in red.
2. **Given** a tool creates a new file, **When** the result is displayed, **Then** all lines are shown as additions.
3. **Given** unchanged lines surround a change, **When** the diff is displayed, **Then** context lines appear in a dimmed style.
4. **Given** a diff exceeds a size threshold, **When** displayed, **Then** the diff is truncated with a summary of omitted lines.
5. ~~**Given** a diff for a file with a known type, **When** displayed, **Then** the diff content is syntax-highlighted.~~ *Deferred — diffs use generic color coding (green/red/dim); syntax highlighting is available for markdown code blocks only.*
6. **[Addition]** **Given** the terminal is at least 160 columns wide and the modified file is not a new file, **When** the diff is displayed, **Then** old and new content render as a side-by-side (two-column) diff instead of the unified layout; below that width, or for newly created files, the unified/inline diff layout is always used.

---

### User Story 3 - Track Resource Consumption via Status Bar (Priority: P1)

A developer monitors the agent's resource usage and state through a persistent status bar. The status bar displays: the current model name, token usage (formatted as human-readable counts like "12.5K"), estimated cost, the agent's state (idle, running, error, or aborted), a retry indicator when retries are in progress, and elapsed session time. This information is always visible at a glance.

**Why this priority**: Resource awareness prevents surprise costs and helps developers understand agent behavior — this is always-visible, non-intrusive information.

**Independent Test**: Can be tested by running an agent interaction and verifying the status bar updates to show the correct model, token count, cost, and state transitions.

**Acceptance Scenarios**:

1. **Given** the agent is idle, **When** the developer looks at the status bar, **Then** the state shows "IDLE" and elapsed session time is displayed.
2. **Given** the agent is generating a response, **When** the status bar updates, **Then** the state shows "RUNNING" and elapsed time increments.
3. **Given** the agent encounters an error, **When** the status bar updates, **Then** the state shows "ERROR."
3a. **Given** the agent is aborted by the user, **When** the status bar updates, **Then** the state shows "ABORTED."
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

A developer manages screen space by collapsing and expanding tool result blocks in the conversation. When a tool result first appears, it is expanded to show full details. After 10 seconds, it auto-collapses to a single-line summary to reduce clutter. The developer can toggle expansion with a keyboard shortcut (F2) and cycle selection between tool blocks with Shift+Left/Right. If the developer has manually expanded a block, it resists auto-collapse — the developer's explicit choice is preserved.

**Why this priority**: Collapsible blocks reduce visual clutter in tool-heavy conversations, but are a usability enhancement rather than core functionality.

**Independent Test**: Can be tested by triggering multiple tool calls, waiting for auto-collapse, then pressing F2 to expand a selected block and verifying it stays expanded.

**Acceptance Scenarios**:

1. **Given** a tool result appears, **When** initially displayed, **Then** it is expanded showing full details.
2. **Given** an expanded tool result, **When** a timeout elapses, **Then** the block auto-collapses to a one-line summary.
3. **Given** a collapsed tool block is selected, **When** F2 is pressed, **Then** the block expands.
4. **Given** the developer has manually expanded a block, **When** the auto-collapse timeout elapses, **Then** the block remains expanded.
5. **Given** multiple tool blocks, **When** Shift+Left/Right is pressed, **Then** selection cycles between tool blocks.

---

### User Story 6 - Approve or Reject Individual Hunks (Priority: P2) **[Addition]**

A developer reviews a proposed file modification hunk by hunk instead of accepting or rejecting the whole write. At the approval prompt for a file write, the developer opens a per-hunk review and is shown one changed region at a time with a progress indicator. For each hunk they apply it, revert it, or apply all remaining hunks at once. When the review finishes, only the approved hunks are written; rejected hunks keep their original content, and the agent is told which hunks were reverted so it does not assume its write landed intact. The developer can cancel the review at any point and fall back to the whole-call prompt.

**Why this priority**: Whole-call approval forces an all-or-nothing choice on a write that may be mostly good — per-hunk review recovers the useful parts of a partially-wrong edit. It is a usability enhancement over the existing approval gate, not a new gate.

**Independent Test**: Can be tested by having the agent propose a multi-hunk write, opening the review, approving some hunks and rejecting others, and verifying the resulting file contains exactly the approved changes.

**Acceptance Scenarios**:

1. **Given** a pending approval for a write that modifies an existing file, **When** the developer opens the per-hunk review, **Then** the first changed hunk is displayed with its position among the total hunk count.
2. **Given** a hunk is displayed, **When** the developer applies it, **Then** the next hunk is displayed for review.
3. **Given** the last hunk has been decided, **When** the review completes, **Then** the pending approval resolves and the review closes.
4. **Given** every hunk was applied, **When** the review completes, **Then** the write proceeds with exactly the content the agent proposed.
5. **Given** every hunk was reverted, **When** the review completes, **Then** the write is rejected and the file is left unchanged.
6. **Given** some hunks were applied and others reverted, **When** the review completes, **Then** the write proceeds with only the approved hunks applied and the reverted hunks retaining their original content.
7. **Given** at least one hunk was reverted, **When** the review completes, **Then** the agent receives a follow-up message identifying which hunks were reverted.
8. **Given** a review is in progress, **When** the developer chooses to apply all remaining hunks, **Then** every undecided hunk is applied and the review completes.
9. **Given** a review is in progress, **When** the developer cancels it, **Then** the review closes with no decisions recorded and the whole-call approval prompt remains pending.
10. **Given** an approval whose write creates a new file, **When** the developer attempts to open a per-hunk review, **Then** no review opens and the whole-call approval prompt remains.

---

### Edge Cases

- **Dozens of concurrent tools**: The tool panel caps its height at 10 lines; excess entries are not visible until earlier ones complete and age out.
- **[Addition]** **Write proposed for a path outside the execution root**: No diff preview is produced, so no per-hunk review is offered and the whole-call approval prompt is used. The write itself is still rejected at execution time by the existing path check.
- **[Addition]** **File changes on disk between the preview and the write**: The preview is a snapshot taken when the approval request is built; the merged content is computed against that snapshot, so a concurrent external edit is overwritten just as it would be by an unreviewed write.
- **[Addition]** **Write whose content is identical to what is on disk**: There are no hunks to review, so no per-hunk review is offered.
- **Binary files or missing newline**: Diffs operate on line-split text content; binary files are not specially handled (they render as raw text lines). Missing trailing newlines are handled by Rust's `str::lines()`.
- **Long model names**: The status bar renders the full model name without truncation; very long names push other elements rightward.
- **No context usage reported**: The context gauge is hidden entirely when `context_budget` is 0.
- **Extremely large tool output**: Tool result summaries are truncated to the first line (max 60 chars) when collapsed. Diff output is capped at 50 lines.
- **Entirely replaced files (no common lines)**: The LCS algorithm returns an empty match set, so all old lines show as removals and all new lines as additions.
- **Cost unavailable**: Cost displays as `$0.0000` when no cost data is provided.
- **Tool hangs indefinitely**: The tool remains in the active list with its elapsed-time counter incrementing until the agent cancels it or the session ends.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The tool panel MUST display an animated spinner for each actively executing tool.
- **FR-002**: The tool panel MUST replace the spinner with a check badge on success or a cross badge on failure.
- **FR-003**: The tool panel MUST auto-hide after 10 seconds when all tools have completed (resolved approvals auto-hide after 2 seconds).
- **FR-004**: The tool panel MUST appear when tools begin executing and hide when idle.
- **FR-005**: Tool result blocks MUST default to expanded and auto-collapse to a one-line summary after 10 seconds.
- **FR-006**: Tool result blocks MUST be toggleable via F2 keyboard shortcut.
- **FR-007**: Tool result blocks MUST support selection cycling via Shift+Left/Shift+Right.
- **FR-008**: User-expanded tool result blocks MUST resist auto-collapse.
- **FR-009**: Inline diffs MUST use unified diff format with additions in green, removals in red, and context lines dimmed.
- **FR-010**: New files MUST be displayed as all-addition diffs.
- **FR-011**: Large diffs MUST be truncated with a summary of omitted lines.
- **FR-012**: ~~Diffs MUST be syntax-highlighted when the file type is recognized.~~ *Deferred — diffs use generic green/red/dim color coding. Syntax highlighting is available for markdown code blocks via `syntect` but not yet applied to diff lines.*
- **FR-013**: The status bar MUST display model name, token usage, estimated cost, agent state, and elapsed time.
- **FR-014**: The status bar MUST show a retry indicator during retry operations.
- **FR-015**: Token counts MUST be formatted in human-readable notation (K for thousands, M for millions).
- **FR-016**: The context window gauge MUST be 10 characters wide and color-coded: green (<60%), yellow (60-85%), red (>85%).
- **FR-017**: The diff computation MUST use a longest-common-subsequence algorithm.
- **FR-018**: **[Addition]** A pending approval for a write to an existing file MUST offer a per-hunk review, presenting one hunk at a time with its position among the total.
- **FR-019**: **[Addition]** Each hunk MUST support apply, revert, and apply-all-remaining decisions, and the review MUST be cancellable back to the whole-call approval prompt with no decisions recorded.
- **FR-020**: **[Addition]** Completing a review with every hunk approved MUST apply exactly the proposed content; with every hunk rejected MUST leave the file unchanged; with a mix MUST apply only the approved hunks.
- **FR-021**: **[Addition]** A hunk that was not explicitly approved MUST be treated as rejected — the review MUST NOT apply a change the developer did not accept.
- **FR-022**: **[Addition]** When at least one hunk is reverted, the agent MUST receive a follow-up message identifying the reverted hunks.
- **FR-023**: **[Addition]** Per-hunk review MUST NOT be offered for new files, for writes with no changes, or when the before-content cannot be safely resolved.

### Key Entities

- **ToolPanel**: A docked region above the conversation area that shows active tools, recently completed tools, pending approvals, and resolved approvals. Uses braille spinner frames. Height capped at 10 lines. Auto-hides when idle. **[Addition]** Each active tool tracks a `streamed_output` buffer of live/incremental output as it arrives; the panel shows the most recent non-empty output line as a truncated preview next to the tool name while it is still running.
- **ToolResultBlock**: A collapsible section in the conversation displaying a tool's output. Has expanded and collapsed states, with auto-collapse behavior and user override.
- **DiffView**: A visual representation of file changes in unified diff format with color-coded additions/removals. Truncates at 50 lines. **[Addition]** Switches to a side-by-side (two-column) layout instead of unified when the terminal is >= 160 columns wide and the file is not newly created.
- **Hunk** **[Addition]**: A maximal contiguous run of changed lines between the before and after content, bounded by unchanged lines. The unit of per-hunk approval.
- **HunkReview** **[Addition]**: The in-progress per-hunk decision session for a pending write approval. Holds the diff, its hunks, a per-hunk decision (apply / revert / undecided), and a cursor for the hunk under review. Resolves the pending approval once every hunk is decided.
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
- **SC-007**: **[Addition]** A developer can accept part of a proposed file write and reject the rest in a single review pass, without hand-editing the file afterward.
- **SC-008**: **[Addition]** The content written after a per-hunk review contains every approved hunk and no rejected hunk; an all-approve review is byte-for-byte identical to the unreviewed write.

## Assumptions

- The TUI scaffold, event loop, and conversation view from specs 025-026 are in place.
- Tool execution events (start, progress, completion, failure) are emitted by the agent event system.
- File content before and after modification is available to compute diffs (provided by the tool result or agent context). **[Addition]** The same before/after content is available on the *approval request*, before the write is applied, which is what makes per-hunk review possible; the approval gate itself (spec 029) is unchanged.
- Token usage and cost data are provided by the agent or adapter layer; the TUI only displays them.
- The context window size (maximum tokens) for the current model is known so that utilization percentage can be calculated.
- The auto-hide and auto-collapse timeouts are hardcoded at 10 seconds (not configurable via TUI config).
