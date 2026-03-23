# Public API Contract: TUI: Tool Panel, Diffs & Status Bar

**Feature**: 027-tui-tools-diffs-status | **Date**: 2026-03-22

## Tool Panel API

### `ToolPanel`

```rust
impl ToolPanel {
    pub const fn new() -> Self;
    pub fn start_tool(&mut self, id: String, name: String);
    pub fn end_tool(&mut self, id: &str, is_error: bool);
    pub fn set_awaiting_approval(&mut self, id: &str, name: &str, arguments: &Value);
    pub fn resolve_approval(&mut self, id: &str, approved: bool);
    pub const fn has_pending_approval(&self) -> bool;
    pub fn tick(&mut self);              // Advance spinner, prune stale entries
    pub const fn is_visible(&self) -> bool;
    pub fn height(&self) -> u16;         // 0 when hidden, 2-10 when visible
    pub fn render(&self, frame: &mut Frame, area: Rect);
}
```

## Diff API

### `DiffData`

```rust
impl DiffData {
    pub fn from_details(details: &serde_json::Value) -> Option<Self>;
}

pub fn render_diff_lines(diff: &DiffData, max_width: u16) -> Vec<Line<'static>>;
```

**Expected `details` JSON shape** (from `WriteFileTool`):
```json
{
  "path": "/path/to/file.rs",
  "is_new_file": false,
  "old_content": "previous content...",
  "new_content": "updated content..."
}
```

## Format Helpers

```rust
pub fn format_tokens(n: u64) -> String;
// <1K: "742", 1K-10K: "4.6K", 10K-1M: "15K", 1M+: "1.2M"

pub fn format_elapsed(start: Instant) -> String;
// <1h: "MM:SS", >=1h: "HH:MM:SS"

pub fn format_context_gauge(tokens_used: u64, budget: u64) -> (String, f32);
// Returns (bar_string, percentage). Bar is 10 chars: "[████░░░░░░]"
// budget=0 returns ("[ no limit ]", 0.0)
```

## Status Bar

```rust
pub fn render(frame: &mut Frame, app: &App, area: Rect);
```

**Segments** (left to right): State badge | PLAN badge | Color mode badge | Model name | ↓input ↑output | $cost | Elapsed | Context gauge | Retry indicator

## Keybindings

| Key | Action | Context |
|-----|--------|---------|
| F2 | Toggle collapse on selected (or last) tool result block | Input or Conversation focus |
| Shift+Left | Select previous tool result block | Input focus |
| Shift+Right | Select next tool result block | Input focus |

## App State Fields (added by this feature)

| Field | Type | Source |
|-------|------|--------|
| `tool_panel` | `ToolPanel` | Tool start/end/approval events |
| `total_input_tokens` | `u64` | Accumulated from agent turn events |
| `total_output_tokens` | `u64` | Accumulated from agent turn events |
| `total_cost` | `f64` | Accumulated from agent turn events |
| `retry_attempt` | `Option<u32>` | Set during retries, cleared on success |
| `context_budget` | `u64` | Set from model config |
| `context_tokens_used` | `u64` | Updated from agent events |
| `selected_tool_block` | `Option<usize>` | Message index of selected tool block |
