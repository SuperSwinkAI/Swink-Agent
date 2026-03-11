# TUI Architecture

**Related Documents:**
- [PRD](../../planning/PRD.md) §16
- [HLD](../HLD.md)
- [TUI Implementation Phases](../../planning/TUI_PHASES.md)

---

## Overview

The TUI (`agent-harness-tui`) is a separate binary crate that provides an interactive terminal interface for the agent harness. It renders streaming conversations, tool executions, and agent state in a full-screen terminal application.

The implementation uses `ratatui` for rendering and `crossterm` for terminal I/O, following an immediate-mode rendering pattern where the entire UI is redrawn each frame from current state. The async event loop uses `crossterm::EventStream` with `tokio::select!` and a dirty flag to avoid unnecessary redraws.

By default the TUI connects to Ollama for LLM inference. Proxy mode (connecting to OpenAI-compatible APIs) is supported via environment variables.

---

## Component Tree

```
App
├── Conversation View (scrollable, flex-grow)
│   ├── User Message Block
│   │   └── Green left border, text content
│   ├── Assistant Message Block
│   │   ├── Cyan left border
│   │   ├── Thinking Section (dimmed)
│   │   ├── Text Content (markdown → styled spans)
│   │   └── Streaming cursor while in-progress
│   ├── Tool Result Block
│   │   └── Yellow left border, success/error content
│   ├── Error Block
│   │   └── Red left border
│   └── System Block
│       └── Magenta left border
├── Tool Panel (conditional, shown during tool execution)
│   ├── Active tool: name, braille spinner, elapsed time
│   └── Completed tool: name, ✓/✗ badge, auto-fades after 3s
├── Input Editor (multi-line, dynamic height 3–10 lines)
│   └── Line number gutter, cursor, Shift+Enter newlines, input history
└── Status Bar
    ├── Left: Token usage (formatted K/M)
    ├── Center: Elapsed time, cost
    └── Right: Retry indicator
```

---

## Module Structure

```
tui/src/
├── main.rs        — Entry point, terminal setup/teardown, agent creation from env vars
├── app.rs         — App state, async event loop, key handling, agent dispatch
├── commands.rs    — Command parsing: hash commands (#help, #clear, #info, #copy,
│                    #copy all, #copy code) and slash commands (/quit, /model,
│                    /thinking, /system, /reset)
├── config.rs      — TuiConfig loaded from ~/.config/agent-harness/tui.toml
│                    Fields: show_thinking, auto_scroll, tick_rate_ms, default_model, theme
├── credentials.rs — Cross-platform keychain integration via `keyring` crate.
│                    Manages API keys for Ollama, OpenAI, Anthropic, and Proxy
│                    providers. Functions: get_credential(), store_credential(),
│                    any_key_configured()
├── session.rs     — Session persistence as JSONL files in
│                    ~/.config/agent-harness/sessions/. SessionMeta tracks id,
│                    model, system_prompt, timestamps, message_count.
│                    Functions: save_session(), load_session(), list_sessions()
├── wizard.rs      — First-run interactive setup wizard. Triggered when no API
│                    keys are configured. Walks user through provider selection
│                    and API key entry
├── format.rs      — format_tokens() (human-readable K/M), format_elapsed()
├── theme.rs       — Color constants and style helpers
└── ui/
    ├── mod.rs           — Layout composition, root render() function
    ├── input.rs         — InputEditor: multi-line editor with cursor navigation,
    │                      Shift+Enter newlines, input history (Up/Down recall),
    │                      dynamic height 3–10, line number gutter
    ├── conversation.rs  — ConversationView: role-colored left borders, auto-scroll,
    │                      manual scroll with "↓ scroll to bottom" indicator,
    │                      markdown rendering, thinking sections, streaming cursor
    ├── markdown.rs      — markdown_to_lines(): headers, bold/italic/code inline,
    │                      fenced code blocks with syntax highlighting,
    │                      bullet/numbered lists, word wrapping
    ├── syntax.rs        — syntect-based highlighting with OnceLock caching,
    │                      integrated into markdown fenced blocks
    ├── status_bar.rs    — Status bar: formatted tokens, elapsed time, cost, retry
    └── tool_panel.rs    — ToolPanel: braille spinner for active tools, ✓/✗ for
                           completed, auto-fade after 3s
```

---

## Event Loop

The TUI runs a single async event loop that multiplexes three event sources using `crossterm::EventStream` and `tokio::select!`. A dirty flag tracks whether state has changed, avoiding unnecessary redraws.

```rust
loop {
    tokio::select! {
        // 1. Terminal events (keyboard, resize)
        Some(event) = terminal_events.next() => {
            handle_terminal_event(event, &mut app);
        }
        // 2. Agent events (streamed via mpsc forwarder task)
        Some(agent_event) = agent_rx.recv() => {
            handle_agent_event(agent_event, &mut app);
        }
        // 3. Tick timer (spinners, elapsed time, tool fade)
        _ = tick_interval.tick() => {
            app.tick();
        }
    }
    // Re-render only if state changed
    if app.dirty {
        terminal.draw(|frame| ui::render(frame, &app))?;
        app.dirty = false;
    }
}
```

Agent integration uses `prompt_stream()` with an mpsc forwarder task that sends `AgentEvent` variants into the event loop. All `AgentEvent` variants are handled: text deltas, thinking deltas, tool calls, tool results, usage, errors, and completion.

---

## Key Bindings

| Key | Action |
|---|---|
| `Enter` | Send message (when input is non-empty) |
| `Shift+Enter` | Insert newline in input editor |
| `Escape` | Abort running agent |
| `Ctrl+C` | Abort agent or quit if idle |
| `Ctrl+Q` | Quit application |
| `Up/Down` | Scroll conversation (conversation focus) / input history (input focus) |
| `Page Up/Down` | Scroll conversation by page |
| `Home` / `Ctrl+A` | Move cursor to start of line |
| `End` / `Ctrl+E` | Move cursor to end of line |
| `Tab` | Cycle focus between Input and Conversation |

Typing any printable character auto-focuses the input editor.

---

## Focus Management

Tab cycles focus between the Input Editor and Conversation View. The focused component renders with a brighter border to indicate selection. Typing any character automatically shifts focus to the input editor.

---

## Rendering Pipeline

1. **State update** — Terminal or agent events mutate `App` state and set the dirty flag
2. **Layout** — `ratatui::Layout` computes widget areas from terminal dimensions
3. **Render** — Each component renders into its allocated `Rect`:
   - Conversation view: iterates messages, renders each with role-colored left border and markdown-formatted content
   - Input editor: renders editable text with line number gutter and cursor
   - Tool panel: renders tool status list with braille spinners or completion badges
   - Status bar: renders formatted token counts, elapsed time, cost, and retry state
4. **Diff** — `ratatui` + `crossterm` handle differential screen updates

---

## Streaming Display

During assistant response streaming:
- `MessageStart` — append a new assistant message block to conversation
- `MessageUpdate(TextDelta)` — append text to the current block, re-render with streaming cursor
- `MessageUpdate(ThinkingDelta)` — append to thinking section (dimmed)
- `MessageUpdate(ToolCallDelta)` — append to tool call argument preview
- `MessageEnd` — finalize the message block, remove streaming cursor
- `ToolExecutionStart` — show tool in tool panel with braille spinner
- `ToolExecutionEnd` — update tool panel with ✓/✗ badge, auto-fade after 3s

The conversation view auto-scrolls to bottom during streaming unless the user has manually scrolled up. When scrolled up, a "↓ scroll to bottom" indicator appears.

---

## Command System

Two command prefixes are supported:

**Hash commands** (processed locally):
- `#help` — show available commands
- `#clear` — clear conversation history
- `#info` — show session info
- `#copy` — copy last assistant message to clipboard
- `#copy all` — copy entire conversation to clipboard
- `#copy code` — copy last code block to clipboard
- `#sessions` — list saved sessions
- `#save` — save current session
- `#load <id>` — load a saved session
- `#keys` — show configured API keys
- `#key <provider> <key>` — set an API key

**Slash commands** (may affect agent state):
- `/quit` — exit the application
- `/model` — show or change the current model
- `/thinking` — toggle thinking display
- `/system` — set the system prompt
- `/reset` — reset the conversation

Clipboard operations use the `arboard` crate.

---

## Configuration

The TUI loads configuration from `~/.config/agent-harness/tui.toml` via `TuiConfig`:

| Field | Type | Description |
|---|---|---|
| `show_thinking` | `bool` | Whether to display thinking sections |
| `auto_scroll` | `bool` | Auto-scroll to bottom on new content |
| `tick_rate_ms` | `u64` | Tick interval for animations |
| `default_model` | `String` | Default model identifier |
| `theme` | `String` | Reserved for future theme switching (loaded but currently unused; colors are hardcoded in theme.rs) |

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

## Logging

The TUI uses `tracing` with `tracing-appender` for file-based logging. Logs are written as daily rolling files to `~/.config/agent-harness/logs/agent-harness.log`. The `tracing-subscriber` layer is configured at startup so that diagnostic output goes to disk rather than interfering with the terminal UI.

---

## Dependencies

| Crate | Version | Purpose |
|---|---|---|
| `agent-harness` | workspace | Core agent library |
| `agent-harness-adapters` | workspace | LLM provider adapters (Ollama, proxy) |
| `ratatui` | 0.29 | Terminal UI framework |
| `crossterm` | 0.28 | Terminal backend (with `event-stream` feature) |
| `tokio` | — | Async runtime |
| `syntect` | 5 | Syntax highlighting for code blocks |
| `futures` | — | Stream utilities for EventStream |
| `arboard` | 3 | Clipboard access |
| `toml` | 0.8 | Configuration file parsing |
| `dirs` | 6 | Platform config directory resolution |
| `serde` | — | Configuration deserialization |
| `keyring` | 3 | Cross-platform keychain integration for API key storage |
| `tracing` | 0.1 | Structured logging facade |
| `tracing-subscriber` | 0.3 | Log output subscriber layer |
| `tracing-appender` | 0.2 | Daily rolling file log output |
