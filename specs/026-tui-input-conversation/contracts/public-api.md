# Public API Contract: TUI: Input & Conversation

**Feature**: 026-tui-input-conversation | **Date**: 2026-03-20

## Module: `swink_agent_tui::ui::input`

```rust
/// Multi-line input editor state.
///
/// Manages a text buffer as a vector of lines, cursor position, dynamic
/// height, and per-session input history with draft preservation.
pub struct InputEditor {
    pub lines: Vec<String>,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub scroll_offset: usize,
    pub history: Vec<Vec<String>>,
    pub history_index: Option<usize>,
    // saved_input: Option<Vec<String>> (private)
}

impl InputEditor {
    /// Create a new empty editor with a single blank line.
    #[must_use]
    pub fn new() -> Self;

    /// Dynamic height for the input area, clamped to 3..=10.
    /// Includes 2 lines for borders.
    #[must_use]
    pub fn height(&self) -> u16;

    /// Insert a character at the cursor position.
    pub fn insert_char(&mut self, c: char);

    /// Insert a newline at the cursor, splitting the current line (Shift+Enter).
    pub fn insert_newline(&mut self);

    /// Delete the character before the cursor. Merges lines at column 0.
    pub fn backspace(&mut self);

    /// Delete the character at the cursor. Merges with next line at end.
    pub fn delete(&mut self);

    /// Move cursor left. Wraps to end of previous line at column 0.
    pub fn move_left(&mut self);

    /// Move cursor right. Wraps to start of next line at end.
    pub fn move_right(&mut self);

    /// Move cursor up one row. Clamps column to target line length.
    pub fn move_up(&mut self);

    /// Move cursor down one row. Clamps column to target line length.
    pub fn move_down(&mut self);

    /// Move cursor to start of current line (Home / Ctrl+A).
    pub fn move_home(&mut self);

    /// Move cursor to end of current line (End / Ctrl+E).
    pub fn move_end(&mut self);

    /// Submit the current input. Returns trimmed text and saves to history.
    /// Returns `None` if the input is empty or whitespace-only.
    /// Clears the editor and resets cursor/history state.
    pub fn submit(&mut self) -> Option<String>;

    /// Navigate to previous history entry.
    /// On first call, saves the current draft. No-op if history is empty
    /// or already at the oldest entry.
    pub fn history_prev(&mut self);

    /// Navigate to next history entry.
    /// Restores the saved draft when moving past the most recent entry.
    pub fn history_next(&mut self);

    /// True if all lines are empty strings.
    pub fn is_empty(&self) -> bool;

    /// True if the buffer contains more than one line.
    pub fn is_multiline(&self) -> bool;

    /// Render the editor widget into the given area.
    ///
    /// - Multi-line mode: displays a line number gutter (right-aligned, 2 digits).
    /// - Focused: positions the terminal cursor at the editor cursor location.
    /// - `status_hint`: optional text shown in the title (e.g., model name).
    pub fn render(&self, frame: &mut Frame, area: Rect, focused: bool, status_hint: &str);
}
```

**Contract**:

### Text Editing
- Characters are inserted at `(cursor_row, cursor_col)`. Column is clamped to line length before insertion.
- `insert_newline()` splits the current line at the cursor: text before stays, text after moves to a new line below.
- `backspace()` at column 0 merges the current line into the previous line's end. At `(0, 0)`, no-op.
- `delete()` at end of line merges the next line into the current line's end. At last position of last line, no-op.

### Cursor Movement
- Left/right wrap across line boundaries. Up/down clamp column to the target line's length.
- `move_home()` sets column to 0. `move_end()` sets column to current line length.

### Submission
- `submit()` joins lines with `\n`, trims the result, and returns `None` if empty.
- On successful submission: text is pushed to `history`, editor resets to a single empty line, `history_index` and `saved_input` are cleared.

### History Navigation
- `history_prev()`: first call saves current `lines` as `saved_input`, sets `history_index` to `history.len() - 1`, and loads that entry. Subsequent calls decrement the index.
- `history_next()`: increments `history_index`. When index exceeds history length, restores `saved_input` and clears `history_index`.
- Editing a recalled entry does not modify the history vector.
- After both `history_prev()` and `history_next()`, cursor moves to end of last line.

### Rendering
- Line numbers appear only in multi-line mode (2+ lines). Format: right-aligned 2-digit number followed by a space (e.g., ` 1 `).
- Dynamic height: `content_lines + 2` (borders), clamped to `[3, 10]`.
- Cursor is positioned within the widget area, accounting for gutter width and scroll offset.

---

## Module: `swink_agent_tui::ui::conversation`

```rust
/// Conversation view state.
///
/// Manages scroll position and auto-scroll behavior for the message list.
pub struct ConversationView {
    pub scroll_offset: usize,
    pub auto_scroll: bool,
    // rendered_lines: usize (private)
}

impl ConversationView {
    /// Create a new view: offset 0, auto-scroll enabled.
    #[must_use]
    pub const fn new() -> Self;

    /// Scroll up by `n` lines. Disengages auto-scroll.
    pub const fn scroll_up(&mut self, n: usize);

    /// Scroll down by `n` lines. Re-engages auto-scroll if at bottom.
    pub const fn scroll_down(&mut self, n: usize, visible_height: usize);

    /// Clamp scroll offset to valid range for current content.
    pub const fn clamp_scroll_offset(&mut self, visible_height: usize);

    /// Jump to bottom and re-engage auto-scroll.
    pub const fn scroll_to_bottom(&mut self, visible_height: usize);

    /// Render the conversation view.
    ///
    /// - `messages`: the full message list to render.
    /// - `focused`: whether the conversation view has keyboard focus.
    /// - `blink_on`: whether the streaming cursor blink is in the "on" phase.
    /// - `selected_tool_block`: index of the currently selected tool block (for F2 inspect).
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        messages: &[DisplayMessage],
        focused: bool,
        blink_on: bool,
        selected_tool_block: Option<usize>,
    );
}
```

**Contract**:

### Scroll Behavior
- `scroll_up(n)` decrements `scroll_offset` by `n` (saturating at 0) and sets `auto_scroll = false`.
- `scroll_down(n, visible_height)` increments `scroll_offset` by `n`. If the new offset reaches or exceeds `rendered_lines - visible_height`, it clamps to that max and sets `auto_scroll = true`.
- `clamp_scroll_offset(visible_height)` ensures `scroll_offset <= rendered_lines - visible_height`.
- `scroll_to_bottom(visible_height)` sets offset to max and `auto_scroll = true`.

### Auto-Scroll
- When `auto_scroll` is true, `render()` sets `scroll_offset` to `rendered_lines - inner_height` each frame.
- Manual scroll up disengages auto-scroll. Scrolling to bottom re-engages it.

### Message Rendering
- Each message is rendered with a colored left border (`│ `) using the role's color.
- A role header line shows the label in bold (e.g., "You", "Assistant", "Tool").
- Message content is rendered via `markdown::markdown_to_lines()` at `inner_width - 2`.
- Streaming messages show a blinking block cursor (`█`) when `blink_on` is true and `is_streaming` is true.
- The cursor disappears when `is_streaming` becomes false (streaming complete).
- A blank line separates consecutive messages.

### Scroll Indicator
- When `auto_scroll` is false and `scroll_offset < max_scroll`, the title includes "scroll to bottom".

---

## Module: `swink_agent_tui::ui::markdown`

```rust
/// Render markdown text into styled `Line`s for ratatui, word-wrapped to `width`.
///
/// Supports: ATX headers (#, ##, ###), **bold**, *italic*, `inline code`,
/// fenced code blocks (with syntax highlighting), bullet lists (- / *),
/// numbered lists (N.), and word-wrapping.
pub fn markdown_to_lines(text: &str, width: u16) -> Vec<Line<'static>>;
```

**Contract**:
- Empty input returns an empty vector.
- Headers are rendered bold; `#` also gets underline. All headers use the heading color.
- Bold text uses `Modifier::BOLD`. Italic text uses `Modifier::ITALIC`.
- Inline code uses the inline code color with `Modifier::BOLD`.
- Fenced code blocks are delegated to `syntax::highlight_code()` with the language label.
- Unclosed code blocks (streaming) are flushed to `highlight_code()` at the end.
- Bullet lists render with Unicode bullet (`\u{2022}`) and 2-space indent.
- Numbered lists preserve the original number with 2-space indent.
- Word-wrapping splits at word boundaries to fit within `width`.
- Empty lines are preserved as empty `Line` values.

---

## Module: `swink_agent_tui::ui::syntax`

```rust
/// Highlight a code block with syntax highlighting.
///
/// Falls back to plain dimmed text if the language isn't recognized.
/// In monochrome mode, skips syntect entirely and renders plain DIM text.
pub fn highlight_code(code: &str, language: &str) -> Vec<Line<'static>>;
```

**Contract**:
- Recognized language: lines are highlighted using syntect with theme colors.
- Unrecognized language or empty string: lines rendered as plain text with `DIM` modifier and mono color.
- Monochrome mode (`color_mode() != Custom`): syntect is skipped entirely.
- Each line is prefixed with 2-space indent.
- `SyntaxSet` and `ThemeSet` are loaded once via `OnceLock` and reused for the process lifetime.
