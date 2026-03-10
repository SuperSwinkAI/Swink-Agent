# TUI Architecture

**Related Documents:**
- [PRD](../../planning/PRD.md) §16
- [HLD](../HLD.md)
- [TUI Implementation Phases](../../planning/TUI_PHASES.md)

---

## Overview

The TUI (`agent-harness-tui`) is a separate binary crate that provides an interactive terminal interface for the agent harness. It renders streaming conversations, tool executions, and agent state in a full-screen terminal application.

The implementation uses `ratatui` for rendering and `crossterm` for terminal I/O, following an immediate-mode rendering pattern where the entire UI is redrawn each frame from current state.

---

## Component Tree

```
App
├── Header Bar
│   └── Model name, thinking level, session info
├── Main Layout (vertical split)
│   ├── Conversation View (scrollable, flex-grow)
│   │   ├── User Message Block
│   │   │   └── Text content with timestamp
│   │   ├── Assistant Message Block
│   │   │   ├── Thinking Block (collapsible, dimmed)
│   │   │   ├── Text Content (markdown → styled spans)
│   │   │   └── Tool Call Block (name, arguments preview)
│   │   └── Tool Result Block
│   │       ├── Success content (text/image placeholder)
│   │       └── Error content (highlighted)
│   └── Tool Panel (conditional, shown during tool execution)
│       ├── Active tool: name, spinner, elapsed time
│       └── Completed tool: name, duration, success/error badge
├── Input Editor (multi-line, word-wrapped)
│   └── Prompt indicator, cursor, line count
└── Status Bar
    ├── Left: Agent state (Idle / Running / Error / Aborted)
    ├── Center: Turn count, message count
    └── Right: Token usage (in/out/cache), cost
```

---

## Module Structure

```
tui/
  Cargo.toml
  src/
    main.rs           — Entry point, CLI args, terminal setup/teardown
    app.rs            — App struct, event loop, focus management
    event.rs          — Event type unifying terminal + agent events
    ui/
      mod.rs          — Layout composition, root render function
      conversation.rs — Conversation view: message blocks, scrolling
      input.rs        — Multi-line input editor component
      tool_panel.rs   — Tool execution status display
      status_bar.rs   — Bottom status bar
      markdown.rs     — Markdown-to-ratatui spans converter
      syntax.rs       — Code block syntax highlighting
    theme.rs          — Color palette, style constants
    config.rs         — TUI configuration (keybindings, colors)
```

---

## Event Loop

The TUI runs a single async event loop that multiplexes three event sources:

```rust
loop {
    tokio::select! {
        // 1. Terminal events (keyboard, mouse, resize)
        Some(event) = terminal_events.next() => {
            handle_terminal_event(event, &mut app);
        }
        // 2. Agent events (from harness subscription)
        Some(agent_event) = agent_events.recv() => {
            handle_agent_event(agent_event, &mut app);
        }
        // 3. Tick timer (for animations: spinners, elapsed time)
        _ = tick_interval.tick() => {
            app.tick();
        }
    }
    // Re-render if state changed
    if app.needs_render() {
        terminal.draw(|frame| ui::render(frame, &app))?;
    }
}
```

---

## Key Bindings

| Key | Action |
|---|---|
| `Enter` | Send message (when input is non-empty) |
| `Shift+Enter` | Insert newline in input editor |
| `Escape` | Cancel running agent / clear input |
| `Ctrl+C` | Abort agent or quit if idle |
| `Ctrl+Q` | Quit application |
| `Up/Down` | Scroll conversation (when not in input) |
| `Page Up/Down` | Scroll conversation by page |
| `Tab` | Cycle focus between components |
| `Ctrl+L` | Force full redraw |

---

## Rendering Pipeline

1. **State update** — Terminal or agent events mutate `App` state
2. **Layout** — `ratatui::Layout` computes widget areas from terminal dimensions
3. **Render** — Each component renders into its allocated `Rect`:
   - Conversation view: iterates messages, renders each as styled paragraphs
   - Input editor: renders editable text with cursor position
   - Tool panel: renders tool status list with spinners
   - Status bar: renders formatted status line
4. **Diff** — `ratatui` + `crossterm` handle differential screen updates

---

## Streaming Display

During assistant response streaming:
- `MessageStart` — append a new assistant message block to conversation
- `MessageUpdate(TextDelta)` — append text to the current block, re-render
- `MessageUpdate(ThinkingDelta)` — append to thinking section (collapsed by default)
- `MessageUpdate(ToolCallDelta)` — append to tool call argument preview
- `MessageEnd` — finalize the message block, apply markdown formatting
- `ToolExecutionStart` — show tool in tool panel with spinner
- `ToolExecutionEnd` — update tool panel with result, hide after delay

The conversation view auto-scrolls to bottom during streaming unless the user has manually scrolled up.

---

## Terminal Setup / Teardown

```
Startup:
1. Enable raw mode (crossterm)
2. Enter alternate screen
3. Enable mouse capture
4. Hide cursor (ratatui manages cursor position)

Shutdown (including panic handler):
1. Disable mouse capture
2. Show cursor
3. Leave alternate screen
4. Disable raw mode
```

A panic hook ensures clean terminal restoration even on crashes.

---

## Dependencies

| Crate | Purpose |
|---|---|
| `ratatui` | Terminal UI framework |
| `crossterm` | Terminal backend |
| `syntect` | Syntax highlighting |
| `tokio` | Async runtime |
| `agent-harness` | Core agent library (workspace dependency) |
