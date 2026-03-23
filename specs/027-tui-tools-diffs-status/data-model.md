# Data Model: TUI: Tool Panel, Diffs & Status Bar

**Feature**: 027-tui-tools-diffs-status | **Date**: 2026-03-22

## Entities

### ToolExecution

Tracks a single tool call through its lifecycle.

| Field | Type | Description |
|-------|------|-------------|
| `id` | `String` | Unique tool call identifier (from agent event) |
| `name` | `String` | Tool name (e.g., "bash", "write_file") |
| `started_at` | `Instant` | When execution began |
| `completed_at` | `Option<Instant>` | When execution finished (None while active) |
| `is_error` | `bool` | Whether the tool failed |

**Lifecycle**: Created in `active` list on tool start event → moved to `completed` list on tool end event → pruned from `completed` after 10 seconds by `tick()`.

### PendingApproval

A tool call awaiting user approval.

| Field | Type | Description |
|-------|------|-------------|
| `id` | `String` | Tool call identifier (for matching resolution) |
| `name` | `String` | Tool name |
| `arguments_summary` | `String` | Redacted, truncated summary of tool arguments |

**Lifecycle**: Created on approval request event → removed on user response (Y/N/A) → replaced by `ResolvedApproval`.

### ResolvedApproval

A recently resolved approval decision (shown briefly for feedback).

| Field | Type | Description |
|-------|------|-------------|
| `approved` | `bool` | Whether the tool was approved |
| `resolved_at` | `Instant` | When the decision was made |

**Lifecycle**: Created on approval resolution → pruned after 2 seconds by `tick()`.

### ToolPanel

Aggregate state for the tool panel UI region.

| Field | Type | Description |
|-------|------|-------------|
| `active` | `Vec<ToolExecution>` | Currently executing tools |
| `completed` | `Vec<ToolExecution>` | Recently completed tools |
| `pending_approvals` | `Vec<PendingApproval>` | Tools awaiting user approval |
| `resolved_approvals` | `Vec<ResolvedApproval>` | Recently resolved approvals |
| `spinner_frame` | `usize` | Current animation frame index (0-9) |

**Visibility rule**: Panel is visible if any of the four collections is non-empty. Height = min(total entries + 2 borders, 10).

### DiffData

Parsed diff information extracted from a tool result's `details` JSON.

| Field | Type | Description |
|-------|------|-------------|
| `path` | `String` | File path that was modified |
| `is_new_file` | `bool` | Whether the file was newly created |
| `old_content` | `String` | Content before modification (empty for new files) |
| `new_content` | `String` | Content after modification |

**Source**: Parsed from `WriteFileTool` result details via `DiffData::from_details(&Value)`. Returns `None` if required fields are missing.

### DisplayMessage (extended fields)

Additional fields on `DisplayMessage` (defined in 026) for this feature.

| Field | Type | Description |
|-------|------|-------------|
| `collapsed` | `bool` | Whether the tool result block is collapsed |
| `summary` | `String` | One-line summary (first line, max 60 chars) |
| `user_expanded` | `bool` | Set when user manually expands — prevents auto-collapse |
| `expanded_at` | `Option<Instant>` | When the block was last expanded (for auto-collapse timing) |
| `diff_data` | `Option<DiffData>` | Parsed diff data if the tool result contains file changes |

### AgentStatus

Enum representing the agent's current state.

| Variant | Display | Color |
|---------|---------|-------|
| `Idle` | `IDLE` | Green |
| `Running` | `RUNNING` | Yellow |
| `Error` | `ERROR` | Red |
| `Aborted` | `ABORTED` | Magenta |

## Relationships

```
ToolPanel 1──* ToolExecution (active)
ToolPanel 1──* ToolExecution (completed)
ToolPanel 1──* PendingApproval
ToolPanel 1──* ResolvedApproval
DisplayMessage 1──? DiffData (optional, only for ToolResult role)
App 1──1 ToolPanel
App 1──1 AgentStatus
App *──1 DisplayMessage
```

## Constants

| Name | Value | Description |
|------|-------|-------------|
| `SPINNER` | 10 braille chars | Animation frames: ⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ |
| `MAX_DIFF_LINES` | 50 | Diff output truncation limit |
| `AUTO_COLLAPSE_SECS` | 10 | Seconds before auto-collapsing tool results |
| Tool panel auto-hide | 10s | Completed tools pruned after 10 seconds |
| Approval auto-hide | 2s | Resolved approvals pruned after 2 seconds |
| Tool panel max height | 10 | Maximum panel height in lines (including borders) |
| Summary max length | 60 | Tool result summary truncation limit in characters |
