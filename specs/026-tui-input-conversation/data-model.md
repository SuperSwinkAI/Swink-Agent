# Data Model: TUI: Input & Conversation

**Feature**: 026-tui-input-conversation | **Date**: 2026-03-20

## Entity: InputEditor (struct, public)

**Location**: `tui/src/ui/input.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `lines` | `Vec<String>` | Lines of text in the editor buffer |
| `cursor_row` | `usize` | Current cursor row (0-indexed) |
| `cursor_col` | `usize` | Current cursor column (0-indexed) |
| `scroll_offset` | `usize` | Vertical scroll offset when content exceeds visible area |
| `history` | `Vec<Vec<String>>` | Previously submitted messages (each is multi-line) |
| `history_index` | `Option<usize>` | Current position in history (`None` = editing new input) |
| `saved_input` | `Option<Vec<String>>` | Draft saved when entering history navigation (private) |

| Method | Signature | Purpose |
|--------|-----------|---------|
| `new()` | `fn() -> Self` | Primary constructor: empty single-line editor |
| `height()` | `fn(&self) -> u16` | Dynamic height clamped to 3..=10 (content lines + 2 for borders) |
| `insert_char()` | `fn(&mut self, char)` | Insert character at cursor position |
| `insert_newline()` | `fn(&mut self)` | Split line at cursor (Shift+Enter) |
| `backspace()` | `fn(&mut self)` | Delete character before cursor; merge lines if at col 0 |
| `delete()` | `fn(&mut self)` | Delete character at cursor; merge with next line if at end |
| `move_left()` | `fn(&mut self)` | Move cursor left; wrap to previous line end |
| `move_right()` | `fn(&mut self)` | Move cursor right; wrap to next line start |
| `move_up()` | `fn(&mut self)` | Move cursor up; clamp column to line length |
| `move_down()` | `fn(&mut self)` | Move cursor down; clamp column to line length |
| `move_home()` | `fn(&mut self)` | Move cursor to start of current line |
| `move_end()` | `fn(&mut self)` | Move cursor to end of current line |
| `submit()` | `fn(&mut self) -> Option<String>` | Submit text, save to history, clear editor. Returns `None` if whitespace-only |
| `history_prev()` | `fn(&mut self)` | Navigate to previous history entry (saves draft on first call) |
| `history_next()` | `fn(&mut self)` | Navigate to next history entry (restores draft at end) |
| `is_empty()` | `fn(&self) -> bool` | True if all lines are empty |
| `is_multiline()` | `fn(&self) -> bool` | True if more than one line exists |
| `render()` | `fn(&self, &mut Frame, Rect, bool, &str)` | Render editor with optional line numbers, cursor positioning |

---

## Entity: ConversationView (struct, public)

**Location**: `tui/src/ui/conversation.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `scroll_offset` | `usize` | Current scroll offset in rendered lines |
| `auto_scroll` | `bool` | Whether auto-scroll is engaged (jumps to bottom each frame) |
| `rendered_lines` | `usize` | Total rendered line count (computed each frame, private) |

| Method | Signature | Purpose |
|--------|-----------|---------|
| `new()` | `const fn() -> Self` | Constructor: offset 0, auto-scroll on |
| `scroll_up()` | `const fn(&mut self, usize)` | Scroll up by N lines; disengages auto-scroll |
| `scroll_down()` | `const fn(&mut self, usize, usize)` | Scroll down by N lines; re-engages auto-scroll if at bottom |
| `clamp_scroll_offset()` | `const fn(&mut self, usize)` | Clamp offset to valid range given visible height |
| `scroll_to_bottom()` | `const fn(&mut self, usize)` | Jump to bottom and re-engage auto-scroll |
| `render()` | `fn(&mut self, &mut Frame, Rect, &[DisplayMessage], bool, bool, Option<usize>)` | Render conversation: role borders, markdown content, streaming cursor, scroll indicator |

**Rendering behavior**:
- Each message gets a colored left border (`│ `) using the role color
- Role header line shows label (You/Assistant/Tool/Error/System) in bold
- Content rendered via `markdown::markdown_to_lines()` for markdown formatting
- Streaming messages show a blinking block cursor (`█`) when `blink_on` is true
- Title shows "scroll to bottom" indicator when auto-scroll is disengaged

---

## Entity: MessageRole (enum, public)

**Location**: `tui/src/app/state.rs`

| Variant | Border Color | Label |
|---------|-------------|-------|
| `User` | Green | "You" |
| `Assistant` | Cyan | "Assistant" (or "Plan" in plan mode) |
| `ToolResult` | Yellow | "Tool" |
| `Error` | Red | "Error" |
| `System` | Magenta | "System" |

**Derives**: `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`

---

## Entity: DisplayMessage (struct, public)

**Location**: `tui/src/app/state.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `role` | `MessageRole` | Determines border color and header label |
| `content` | `String` | Message text (may contain markdown) |
| `thinking` | `Option<String>` | Thinking section text (rendered dimmed) |
| `is_streaming` | `bool` | Whether the message is still being streamed (shows cursor) |
| `collapsed` | `bool` | Whether this tool result is collapsed |
| `summary` | `String` | One-line summary for collapsed display |
| `user_expanded` | `bool` | Whether user manually expanded (prevents auto-collapse) |
| `expanded_at` | `Option<Instant>` | When expanded (for auto-collapse timing) |
| `plan_mode` | `bool` | Whether produced in plan mode (changes assistant label/color) |
| `diff_data` | `Option<DiffData>` | Diff data for file modification results |

**Derives**: `Debug`, `Clone`

---

## Entity: InputHistory (logical, embedded in InputEditor)

Not a separate struct. History behavior is implemented directly on `InputEditor` via:
- `history: Vec<Vec<String>>` — ordered list of submitted messages
- `history_index: Option<usize>` — navigation position
- `saved_input: Option<Vec<String>>` — in-progress draft preserved during navigation
- `history_prev()` / `history_next()` — navigation methods

---

## Entity: MarkdownRenderer (logical, module-level functions)

**Location**: `tui/src/ui/markdown.rs`

Not a struct. Markdown rendering is a stateless function pipeline:

| Function | Signature | Purpose |
|----------|-----------|---------|
| `markdown_to_lines()` | `fn(&str, u16) -> Vec<Line<'static>>` | Public entry point: parse markdown text into styled lines at given width |
| `parse_inline()` | `fn(&str, Style) -> Vec<Span<'static>>` | Parse inline formatting (bold, italic, code) within a single line (private) |
| `wrap_spans()` | `fn(Vec<Span<'static>>, usize) -> Vec<Vec<Span<'static>>>` | Word-wrap styled spans to fit width (private) |
| `split_preserving_spaces()` | `fn(&str) -> Vec<String>` | Split text into words keeping spaces attached (private) |

**Block-level state machine** in `markdown_to_lines()`:
- Tracks `in_code_block` / `code_lang` / `code_buffer` for fenced code blocks
- Dispatches to `syntax::highlight_code()` on code block close (or flush on unclosed block)
- Detects headers (`#`, `##`, `###`), bullet lists (`- `, `* `), numbered lists (`N. `)

---

## Entity: SyntaxHighlighter (logical, module-level functions)

**Location**: `tui/src/ui/syntax.rs`

Not a struct. Syntax highlighting is a stateless function with cached resources:

| Function | Signature | Purpose |
|----------|-----------|---------|
| `highlight_code()` | `fn(&str, &str) -> Vec<Line<'static>>` | Public entry point: highlight code block with language label |
| `syntax_set()` | `fn() -> &'static SyntaxSet` | `OnceLock`-cached syntax definitions (private) |
| `theme_set()` | `fn() -> &'static ThemeSet` | `OnceLock`-cached color themes (private) |
| `to_ratatui_color()` | `const fn(syntect::Color) -> ratatui::Color` | Color conversion helper (private) |

**Fallback behavior**:
- Monochrome mode: skip syntect, render plain DIM text
- Unrecognized language: fall back to plain dimmed monospace
- Empty language string: plain dimmed monospace

---

## Relationship Diagram

```text
App (state.rs)
  │
  ├── InputEditor (ui/input.rs)
  │     ├── lines: Vec<String>          ── text buffer
  │     ├── cursor_row/col              ── cursor state
  │     ├── history: Vec<Vec<String>>   ── input history
  │     ├── submit() ──► Option<String> ── submitted text sent to agent
  │     └── render() ──► Frame          ── renders with line numbers + cursor
  │
  ├── ConversationView (ui/conversation.rs)
  │     ├── scroll_offset/auto_scroll   ── scroll state
  │     ├── render()
  │     │     ├── DisplayMessage.role ──► role color + border
  │     │     ├── markdown_to_lines() ──► styled content lines
  │     │     │     └── syntax::highlight_code() ──► highlighted code blocks
  │     │     ├── is_streaming + blink_on ──► streaming cursor (█)
  │     │     └── auto_scroll ──► scroll-to-bottom indicator
  │     └── scroll_up/down/to_bottom ──► scroll control
  │
  ├── Vec<DisplayMessage> (state.rs)
  │     └── DisplayMessage
  │           ├── role: MessageRole     ── User/Assistant/Tool/Error/System
  │           ├── content: String       ── markdown text
  │           ├── is_streaming: bool    ── streaming indicator
  │           └── thinking/collapsed/diff_data ── auxiliary display state
  │
  └── Focus (state.rs)
        ├── Input ──► key events go to InputEditor
        └── Conversation ──► key events go to ConversationView scroll
```
