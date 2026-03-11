# CLAUDE.md — TUI UI Components

## Scope

`tui/src/ui/` — Rendering components for the terminal UI. Each module is a self-contained widget rendered into a ratatui `Rect`.

## References

- **PRD:** §16.3 (Rendering), §16.4 (Interaction)
- **Architecture:** `docs/architecture/tui/README.md`

## Component Layout

`mod.rs` — Central `render()` function builds layout dynamically:
- Constraints vec adapts based on component visibility (tool panel height, input height).
- Tool panel is conditional — only rendered when tools are active or recently completed.
- Layout splits the frame vertically: conversation (flex-grow), tool panel (optional), input, status bar.

## Components

| File | Widget | Key behavior |
|---|---|---|
| `conversation.rs` | ConversationView | Scrollable message history, role-colored borders, auto-scroll |
| `input.rs` | InputEditor | Multi-line (3–10 lines), cursor, history, gutter |
| `tool_panel.rs` | ToolPanel | Braille spinner, checkmark/cross badges, 3s auto-fade |
| `status_bar.rs` | StatusBar | Tokens (K/M format), elapsed time, cost, retry indicator |
| `markdown.rs` | (rendering helper) | Line-by-line state machine parser |
| `syntax.rs` | (rendering helper) | syntect-based highlighting with OnceLock caching |

## Lessons Learned

- **Markdown is parsed line-by-line** — `in_code_block` flag tracks fenced blocks. Inline parsing (`parse_inline`) uses `char_indices().peekable()` to detect backticks, asterisks for code/italic/bold. Word-wrap preserves style across line breaks by splitting text within spans.
- **syntect caches are static `OnceLock`** — `SyntaxSet` and `ThemeSet` load once on first `highlight_code` call, zero-copy after. Theme is hardcoded to `base16-ocean.dark`.
- **Tool panel auto-fade** — `tick()` retains completed tools for 3 seconds. `height()` returns 0 when nothing is visible, causing the layout to reclaim the space. Panel caps at 8 lines max.
- **Auto-scroll disengages on manual scroll** — `scroll_up()` sets `auto_scroll = false`. `scroll_down()` re-engages it when user scrolls to bottom. Title shows "scroll to bottom" hint when disengaged.
- **Thinking sections are dimmed, not collapsible** — rendered with dimmed style. No expand/collapse toggle exists. This was a QA finding — docs previously claimed "collapsible" but code never implemented it.
- **Input height is dynamic** — expands from 3 to 10 lines based on content. `height()` method returns the clamped value, which mod.rs uses for layout constraints.
