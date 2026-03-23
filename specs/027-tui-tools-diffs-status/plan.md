# Implementation Plan: TUI: Tool Panel, Diffs & Status Bar

**Branch**: `027-tui-tools-diffs-status` | **Date**: 2026-03-22 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/027-tui-tools-diffs-status/spec.md`

## Summary

Implement the tool execution visibility layer and resource monitoring components for the TUI. This includes: a tool panel docked above the conversation that shows active tools with animated braille spinners and completed tools with success/failure badges (auto-hiding after 10 seconds); inline unified diffs for file modifications using an LCS algorithm with green/red/dim color coding and 50-line truncation; collapsible tool result blocks with F2 toggle, Shift+Left/Right selection cycling, 10-second auto-collapse, and user-expanded override; a persistent status bar showing model name, token usage (K/M notation), cost, agent state (IDLE/RUNNING/ERROR/ABORTED), retry indicator, and elapsed session time; and a 10-character context window gauge with green/yellow/red color thresholds at 60%/85%.

## Technical Context

**Language/Version**: Rust 1.88 (edition 2024)
**Primary Dependencies**: `ratatui` 0.30, `crossterm` 0.29 (event-stream), `syntect` 5 (syntax highlighting for code blocks), `swink-agent` (core types — `Agent`, `ToolApproval`, `ToolApprovalRequest`, event system)
**Storage**: N/A (all state is in-memory per session)
**Testing**: `cargo test -p swink-agent-tui`; unit tests for tool panel lifecycle, diff computation, format helpers, conversation scroll/collapse behavior; integration tests in `tui/tests/ac_tui.rs`
**Target Platform**: Any terminal (macOS, Linux, Windows via crossterm)
**Project Type**: Library + binary (TUI crate)
**Performance Goals**: Tool panel updates within one render frame of event arrival; diff computation completes without blocking the event loop for files under 10K lines
**Constraints**: `#[forbid(unsafe_code)]`; no provider-specific types; ratatui immediate-mode rendering; dirty-flag optimization (redraw only when state changes)

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| # | Principle | Status | Notes |
|---|-----------|--------|-------|
| I | Library-First | PASS | All components (`ToolPanel`, `DiffData`, `render_diff_lines`, `format_tokens`, `format_elapsed`, `format_context_gauge`, `status_bar::render`) are public types/functions in the `swink-agent-tui` crate. Independently testable with unit tests. |
| II | Test-Driven Development | PASS | Unit tests cover tool panel lifecycle (start/end/visibility/height capping), diff computation (LCS, new files, modifications, identical content, truncation), format helpers (token formatting boundaries, elapsed time, gauge math), and conversation collapse behavior. Integration tests verify message role distinctness, DiffData round-trip, and gauge threshold math. |
| III | Efficiency & Performance | PASS | Tool panel prunes completed entries in `tick()` — O(n) scan on small lists. LCS diff uses standard DP (O(m*n) for m,n line counts) — acceptable for file diffs. Format helpers are pure functions with no allocations beyond the returned string. Dirty-flag prevents unnecessary redraws. |
| IV | Leverage the Ecosystem | PASS | Uses `ratatui` for terminal rendering, `crossterm` for input events. Diff computation is hand-rolled (LCS is ~35 lines) — correct choice since no Rust diff crate provides ratatui-native `Line`/`Span` output. `syntect` reused from 026 for code blocks. |
| V | Provider Agnosticism | PASS | Status bar receives token counts, cost, and model name as plain values from `App` state. No provider-specific types. Tool events arrive via the agent event system's generic callbacks. |
| VI | Safety & Correctness | PASS | `#[forbid(unsafe_code)]` at crate root. Diff truncation prevents unbounded output. Tool panel height capped at 10 lines. Scroll offsets clamped. `redact_sensitive_values` applied to tool arguments before display in approval prompts. |

## Project Structure

### Documentation (this feature)

```text
specs/027-tui-tools-diffs-status/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── public-api.md
└── spec.md
```

### Source Code (repository root)

```text
tui/src/
├── format.rs            # format_tokens, format_elapsed, format_context_gauge
├── ui/
│   ├── mod.rs           # Layout orchestration — conditional tool panel rendering
│   ├── tool_panel.rs    # ToolPanel, ToolExecution, PendingApproval, ResolvedApproval
│   ├── diff.rs          # DiffData, render_diff_lines, compute_lcs, truncate_line
│   ├── status_bar.rs    # render() — status bar with all segments
│   ├── conversation.rs  # ConversationView — collapse/expand, diff integration
│   ├── help_panel.rs    # F2 documented as "Collapse tool"
│   └── syntax.rs        # highlight_code (reused from 026, not applied to diffs)
├── app/
│   ├── state.rs         # AgentStatus, OperatingMode, DisplayMessage fields
│   ├── event_loop.rs    # F2 handler, Shift+Left/Right cycling
│   ├── lifecycle.rs     # tick() auto-collapse, toggle_collapse, select_prev/next_tool_block
│   └── agent_bridge.rs  # Tool event handling, DiffData extraction, summary generation
└── theme.rs             # diff_add_color, diff_remove_color, context_green/yellow/red, status colors
```

**Structure Decision**: All source files live within the existing `tui` crate. New files for this feature: `ui/tool_panel.rs`, `ui/diff.rs`, `ui/status_bar.rs`, `format.rs`. Extended files: `app/state.rs` (new fields on `DisplayMessage`, `AgentStatus` enum), `app/event_loop.rs` (F2/Shift+arrow handlers), `app/lifecycle.rs` (tick auto-collapse, toggle/select methods), `app/agent_bridge.rs` (tool event processing, diff extraction), `theme.rs` (diff and status colors), `ui/mod.rs` (layout with tool panel region).

## Design Decisions

### Tool Panel Architecture

The tool panel is a **docked region** above the conversation area, not inline or floating. This keeps layout predictable — the conversation area flexes to fill remaining space after the tool panel, input, and status bar claim their fixed heights.

**Rendering order in layout** (top to bottom):
1. Tool panel (0 or 2-10 lines, conditional)
2. Conversation (flex-grow)
3. Input editor (3-10 lines)
4. Status bar (1 line)

The tool panel tracks four collections: `active` (spinner), `completed` (badge), `pending_approvals` (warning icon + Y/n prompt), `resolved_approvals` (brief confirmation). The `tick()` method prunes completed tools after 10 seconds and resolved approvals after 2 seconds.

### Diff Computation

Diffs use a standard LCS dynamic programming algorithm. The DP table is `O(m*n)` in memory where m and n are line counts. This is acceptable because file diffs in agent tool results are typically small (<1000 lines). The diff output is capped at 50 lines to prevent flooding the conversation.

**DiffData** is extracted from the tool result's `details` JSON field, which `WriteFileTool` populates with `path`, `is_new_file`, `old_content`, and `new_content`.

### Status Bar Segments

Left-to-right layout:
1. State badge (colored background: IDLE/RUNNING/ERROR/ABORTED)
2. Optional PLAN mode badge
3. Optional color mode badge (MONO-W/MONO-B)
4. Model name (dimmed)
5. Token usage (↓input ↑output)
6. Cost ($x.xxxx)
7. Elapsed time (dimmed, MM:SS or HH:MM:SS)
8. Context gauge (colored bar + percentage, hidden when budget is 0)
9. Retry indicator (when active)

### Auto-Collapse Behavior

Tool result blocks start expanded with `expanded_at = Some(Instant::now())`. The `tick()` method checks elapsed time and collapses blocks after 10 seconds unless `user_expanded` is set. When the user presses F2, `toggle_collapse()` flips both `collapsed` and `user_expanded`, ensuring manual expansion persists.

## Complexity Tracking

No constitution violations. All components fit within the existing `tui` crate boundary.
