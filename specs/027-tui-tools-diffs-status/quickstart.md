# Quickstart: TUI: Tool Panel, Diffs & Status Bar

**Feature**: 027-tui-tools-diffs-status | **Date**: 2026-03-22

## Overview

This feature adds tool execution visibility, inline file diffs, collapsible tool result blocks, and resource monitoring to the TUI. It builds on the scaffold (025) and input/conversation (026) features.

## Prerequisites

- Specs 025 (TUI scaffold) and 026 (input & conversation) implemented
- Agent event system emitting tool start/end events
- `WriteFileTool` populating `details` JSON with `path`, `is_new_file`, `old_content`, `new_content`

## New Files

| File | Purpose |
|------|---------|
| `tui/src/ui/tool_panel.rs` | Tool panel component — spinners, badges, approvals |
| `tui/src/ui/diff.rs` | DiffData parsing, LCS computation, diff rendering |
| `tui/src/ui/status_bar.rs` | Status bar rendering — all segments |
| `tui/src/format.rs` | Format helpers — tokens, elapsed time, context gauge |

## Modified Files

| File | Changes |
|------|---------|
| `tui/src/app/state.rs` | Add `collapsed`, `summary`, `user_expanded`, `expanded_at`, `diff_data` to `DisplayMessage`; add `AgentStatus::Aborted`; add context/retry/selection fields to `App` |
| `tui/src/app/event_loop.rs` | Add F2 handler, Shift+Left/Right handler |
| `tui/src/app/lifecycle.rs` | Add `tick()` auto-collapse logic, `toggle_collapse()`, `select_prev/next_tool_block()` |
| `tui/src/app/agent_bridge.rs` | Extract DiffData from tool results, generate summaries, handle tool panel events |
| `tui/src/ui/mod.rs` | Add tool panel region to layout, conditional rendering |
| `tui/src/ui/conversation.rs` | Render collapsed/expanded tool blocks with indicators, integrate diff rendering |
| `tui/src/theme.rs` | Add diff colors, status colors, context gauge colors |
| `tui/src/ui/help_panel.rs` | Document F2 as "Collapse tool" |

## Key Integration Points

### Tool Panel ↔ Agent Events

```
agent_bridge.rs receives ToolStart event → tool_panel.start_tool(id, name)
agent_bridge.rs receives ToolEnd event   → tool_panel.end_tool(id, is_error)
agent_bridge.rs receives ApprovalRequest → tool_panel.set_awaiting_approval(...)
event_loop.rs receives Y/N/A keypress   → tool_panel.resolve_approval(...)
```

### Diff Rendering ↔ Tool Results

```
agent_bridge.rs receives TurnEnd with ToolResult
  → extracts details JSON from result
  → DiffData::from_details(&details)
  → stores in DisplayMessage.diff_data
conversation.rs renders ToolResult message
  → if diff_data.is_some() → render_diff_lines(diff, width)
```

### Status Bar ↔ App State

```
status_bar::render reads from App:
  - app.status (AgentStatus enum)
  - app.model_name
  - app.total_input_tokens, app.total_output_tokens
  - app.total_cost
  - app.session_start (for elapsed time)
  - app.context_budget, app.context_tokens_used
  - app.retry_attempt
  - app.operating_mode
```

## Build & Test

```bash
cargo test -p swink-agent-tui           # All TUI tests
cargo run -p swink-agent-tui            # Launch TUI
cargo clippy -p swink-agent-tui -- -D warnings
```
