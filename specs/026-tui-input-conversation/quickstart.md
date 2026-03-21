# Quickstart: TUI: Input & Conversation

**Feature**: 026-tui-input-conversation | **Date**: 2026-03-20

## Prerequisites

- Rust 1.88+ (edition 2024)
- `swink-agent` core crate available as a path dependency
- Terminal emulator with truecolor support (for syntax highlighting)

## Build & Test

```bash
# Build the TUI crate
cargo build -p swink-agent-tui

# Run all TUI tests
cargo test -p swink-agent-tui

# Run specific component tests
cargo test -p swink-agent-tui input      # InputEditor tests
cargo test -p swink-agent-tui conversation  # ConversationView tests
cargo test -p swink-agent-tui markdown   # Markdown renderer tests

# Full workspace check
cargo test --workspace
cargo clippy --workspace -- -D warnings

# Launch the TUI (auto-loads .env)
cargo run -p swink-agent-tui
```

## Usage Examples

### InputEditor — Compose and Submit

```rust
use swink_agent_tui::ui::input::InputEditor;

let mut editor = InputEditor::new();

// Type a message
editor.insert_char('H');
editor.insert_char('i');

// Multi-line: Shift+Enter inserts newline
editor.insert_newline();
editor.insert_char('!');

// Submit: returns trimmed text, clears editor, saves to history
let text = editor.submit();
assert_eq!(text, Some("Hi\n!".to_string()));
assert!(editor.is_empty());

// Recall last message with history_prev
editor.history_prev();
assert_eq!(editor.lines, vec!["Hi".to_string(), "!".to_string()]);
```

### InputEditor — Cursor Movement

```rust
let mut editor = InputEditor::new();
editor.insert_char('a');
editor.insert_char('b');
editor.insert_char('c');

editor.move_home();      // cursor at column 0
editor.move_end();       // cursor at column 3
editor.move_left();      // cursor at column 2
editor.backspace();      // deletes 'b', text is "ac"
```

### ConversationView — Scroll Control

```rust
use swink_agent_tui::ui::conversation::ConversationView;

let mut view = ConversationView::new();
assert!(view.auto_scroll); // starts with auto-scroll on

// Manual scroll up disengages auto-scroll
view.scroll_up(5);
assert!(!view.auto_scroll);

// Scrolling back to bottom re-engages
view.scroll_to_bottom(20); // 20 = visible height
assert!(view.auto_scroll);
```

### Markdown Rendering

```rust
use swink_agent_tui::ui::markdown::markdown_to_lines;

let md = "# Hello\n\nThis is **bold** and *italic*.\n\n```rust\nlet x = 42;\n```";
let lines = markdown_to_lines(md, 80);
// lines[0] = styled header "Hello" (bold + underline)
// lines[2] = paragraph with bold/italic spans
// lines[4+] = syntax-highlighted Rust code
```

### Syntax Highlighting

```rust
use swink_agent_tui::ui::syntax::highlight_code;

// Recognized language: syntax-highlighted output
let lines = highlight_code("fn main() {}", "rust");
assert!(!lines.is_empty());

// Unrecognized language: plain dimmed text
let lines = highlight_code("some code", "nonexistent");
assert!(!lines.is_empty()); // still renders, just without color
```

## Key Files

| File | Purpose |
|------|---------|
| `tui/src/ui/input.rs` | `InputEditor`: multi-line text editing, cursor, history, rendering |
| `tui/src/ui/conversation.rs` | `ConversationView`: scroll state, role borders, streaming cursor, auto-scroll |
| `tui/src/ui/markdown.rs` | `markdown_to_lines`: markdown parsing, inline formatting, word-wrapping |
| `tui/src/ui/syntax.rs` | `highlight_code`: syntect-based syntax highlighting with `OnceLock` caches |
| `tui/src/app/state.rs` | `DisplayMessage`, `MessageRole`, `Focus` — shared state types |
| `tui/src/app/event_loop.rs` | Key event dispatch: routes input to `InputEditor` or `ConversationView` scroll |
| `tui/src/theme.rs` | Color functions for role borders, headings, inline code, etc. |

## Keybindings (Input Editor Focused)

| Key | Action |
|-----|--------|
| Characters | Insert at cursor |
| Enter | Submit message |
| Shift+Enter | Insert newline |
| Backspace | Delete before cursor |
| Delete | Delete at cursor |
| Left/Right | Move cursor horizontally (wraps at line boundaries) |
| Up/Down | Move cursor vertically (or navigate history when editor is empty) |
| Home / Ctrl+A | Move to start of line |
| End / Ctrl+E | Move to end of line |

## Keybindings (Conversation View Focused)

| Key | Action |
|-----|--------|
| Up/Down | Scroll by 1 line |
| PageUp/PageDown | Scroll by page |
