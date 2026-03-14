# TUI Architecture

**Related Documents:**
- [PRD](../../planning/PRD.md) §16
- [HLD](../HLD.md)
- [TUI Implementation Phases](../../planning/TUI_PHASES.md)

---

## Overview

The TUI (`swink-agent-tui`) is a separate binary crate that provides an interactive terminal interface for the swink agent. It renders streaming conversations, tool executions, and agent state in a full-screen terminal application.

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
│   │   ├── Yellow left border, success/error content
│   │   └── DiffView (for file modifications)
│   ├── Error Block
│   │   └── Red left border
│   └── System Block
│       └── Magenta left border
├── Help Panel (F1 toggle, right side, fixed 34-col width)
│   ├── Key bindings reference
│   ├── # Commands reference
│   └── / Commands reference
├── Tool Panel (conditional, shown during tool execution)
│   ├── Active tool: name, braille spinner, elapsed time
│   └── Completed tool: name, ✓/✗ badge, auto-fades after 10s
├── Input Editor (multi-line, dynamic height 3–10 lines)
│   └── Line number gutter, cursor, Shift+Enter newlines, input history
└── Status Bar
    ├── Left: Token usage (formatted K/M)
    ├── Center: Elapsed time, cost
    ├── Right: Retry indicator
    └── Context Gauge: fill % bar (green/yellow/red)
```

---

## Module Structure

```
tui/src/
├── main.rs        — Entry point, terminal setup/teardown, agent creation from env vars
├── app.rs         — App state, async event loop, key handling, agent dispatch
├── commands.rs    — Command parsing: hash commands (#help, #clear, #info, #copy,
│                    #copy all, #copy code) and slash commands (/quit,
│                    /thinking, /system, /reset)
├── config.rs      — TuiConfig loaded from ~/.config/swink-agent/tui.toml
│                    Fields: show_thinking, auto_scroll, tick_rate_ms, default_model,
│                    theme, color_mode, editor_command
├── credentials.rs — Cross-platform keychain integration via `keyring` crate.
│                    Manages API keys for Ollama, OpenAI, Anthropic, and Proxy
│                    providers. Functions: credential(), store_credential(),
│                    any_key_configured()
├── session.rs     — Re-exports from swink-agent-memory: JsonlSessionStore,
│                    SessionStore trait. Session persistence (JSONL files in
│                    ~/.config/swink-agent/sessions/) is implemented in the
│                    memory crate. See memory/docs/architecture/ for details
├── wizard.rs      — First-run interactive setup wizard. Triggered when no API
│                    keys are configured. Walks user through provider selection
│                    and API key entry
├── format.rs      — format_tokens() (human-readable K/M), format_elapsed()
├── editor.rs      — External editor integration: suspend TUI, open $EDITOR,
│                    submit content on close
├── theme.rs       — ColorMode system (Custom/MonoWhite/MonoBlack), color resolution
│                    functions, and style helpers
└── ui/
    ├── mod.rs           — Layout composition, root render() function
    ├── input.rs         — InputEditor: multi-line editor with cursor navigation,
    │                      Shift+Enter newlines, input history (Up/Down recall),
    │                      dynamic height 3–10, line number gutter
    ├── help_panel.rs    — HelpPanel: F1-toggled side panel with key bindings and
    │                      commands reference. Fixed 34-col width, hidden by default.
    │                      Startup hint ("Press F1 for help.") shown on first launch.
    ├── conversation.rs  — ConversationView: role-colored left borders, auto-scroll,
    │                      manual scroll with "↓ scroll to bottom" indicator,
    │                      markdown rendering, thinking sections, streaming cursor
    ├── markdown.rs      — markdown_to_lines(): headers, bold/italic/code inline,
    │                      fenced code blocks with syntax highlighting,
    │                      bullet/numbered lists, word wrapping
    ├── syntax.rs        — syntect-based highlighting with OnceLock caching,
    │                      integrated into markdown fenced blocks;
    │                      monochrome early-return skips syntect in mono modes
    ├── status_bar.rs    — Status bar: formatted tokens, elapsed time, cost, retry
    ├── tool_panel.rs    — ToolPanel: braille spinner for active tools, ✓/✗ for
    │                      completed, auto-fade after 10s
    └── diff.rs          — DiffView: syntax-highlighted unified/side-by-side diffs
                           with per-hunk approve/reject for file modifications
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
| `Shift+Tab` | Toggle between Plan and Execute mode |
| `F1` | Toggle help side panel |
| `F2` | Expand/collapse selected tool result block |
| `F3` | Cycle color mode (Custom → MonoWhite → MonoBlack) |
| `F4` | Cycle model (applied on next send) |
| `Shift+←` / `Shift+→` | Select previous/next tool block |
| `y` / `n` (in diff view) | Approve/reject individual diff hunk |
| `a` (in diff view) | Approve all remaining diff hunks |

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
- `ToolExecutionEnd` — update tool panel with ✓/✗ badge, auto-fade after 10s

The conversation view auto-scrolls to bottom during streaming unless the user has manually scrolled up. When scrolled up, a "↓ scroll to bottom" indicator appears.

---

## Command System

Two command prefixes are supported:

**Hash commands** (processed locally):
- `#help` — toggle help side panel
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
- `#approve smart` — enable smart approval mode (auto-approve reads, prompt for writes)

**Slash commands** (may affect agent state):
- `/quit` — exit the application
- `/thinking` — toggle thinking display
- `/system` — set the system prompt
- `/reset` — reset the conversation
- `/editor` — open external editor for prompt composition
- `/plan` — toggle plan mode (read-only analysis)

Clipboard operations use the `arboard` crate.

---

## Configuration

The TUI loads configuration from `~/.config/swink-agent/tui.toml` via `TuiConfig`:

| Field | Type | Description |
|---|---|---|
| `show_thinking` | `bool` | Whether to display thinking sections |
| `auto_scroll` | `bool` | Auto-scroll to bottom on new content |
| `tick_rate_ms` | `u64` | Tick interval for animations |
| `default_model` | `String` | Default model identifier |
| `theme` | `String` | Reserved for future theme switching |
| `color_mode` | `String` | Color mode: `"custom"` (default), `"mono-white"`, or `"mono-black"`. Can be cycled at runtime with F3 |
| `editor_command` | `Option<String>` | Override for external editor (defaults to `$EDITOR` / `$VISUAL` / `vi`) |

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

The TUI uses `tracing` with `tracing-appender` for file-based logging. Logs are written as daily rolling files to `~/.config/swink-agent/logs/swink-agent.log`. The `tracing-subscriber` layer is configured at startup so that diagnostic output goes to disk rather than interfering with the terminal UI.

---

## Dependencies

| Crate | Version | Purpose |
|---|---|---|
| `swink-agent` | workspace | Core agent library |
| `swink-agent-adapters` | workspace | LLM provider adapters (Ollama, proxy) |
| `swink-agent-memory` | workspace | Session persistence, compaction strategies |
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

---

## Inline Diff View

**Related:** [PRD §16.6](../../planning/PRD.md#166-inline-diff-view)

When the agent modifies a file via `WriteFileTool`, the TUI renders the change as a syntax-highlighted diff instead of displaying raw tool output. Users can approve or reject individual hunks before changes are finalized.

### Module

New file: `tui/src/ui/diff.rs` — the `DiffView` component.

### Data Model

```
DiffBlock
├── file_path: String
├── hunks: Vec<DiffHunk>
│   ├── old_start: usize
│   ├── new_start: usize
│   ├── old_lines: Vec<String>
│   ├── new_lines: Vec<String>
│   └── status: HunkStatus (Pending | Approved | Rejected)
└── layout: DiffLayout (Unified | SideBySide)
```

### Layout Modes

- **Unified** (default): `+`/`-` prefixed lines with green/red backgrounds, 3-line context around each hunk
- **Side-by-side**: Two equal columns (old | new) when `terminal_width >= 160`. Falls back to unified on narrower terminals

### Integration

- Triggered by `ToolExecutionEnd` events for `WriteFileTool`
- The tool result carries diff data (old content vs. new content)
- `app.rs` converts the tool result into a `DiffBlock` attached to the corresponding `DisplayMessage`

### Interaction

```
Arrow Up/Down  — Navigate between hunks
y              — Approve current hunk
n              — Reject current hunk
a              — Approve all remaining hunks
Esc            — Exit diff view
```

Rejected hunks generate a follow-up tool result message to the agent explaining which changes were reverted.

---

## Context Window Progress Bar

**Related:** [PRD §16.7](../../planning/PRD.md#167-context-window-progress-bar)

A compact gauge in the status bar visualizing how full the context window is. Helps users anticipate when context overflow or compaction will occur.

### Rendering

Renders as a 10-character bar with Unicode block characters and a percentage label:

```
[████████░░] 82%
```

Color transitions based on fill percentage:
- Green (`context_green()`): < 60%
- Yellow (`context_yellow()`): 60–85%
- Red (`context_red()`): > 85%

### Data Flow

- New fields on `App`: `context_fill_pct: f32`, `context_budget: usize`
- Updated on `TurnEnd` events using the `estimate_tokens` heuristic (chars / 4)
- Model context window size sourced from agent configuration or adapter metadata

### Module Changes

- `tui/src/ui/status_bar.rs` — add gauge widget segment between cost and retry indicator
- `tui/src/theme.rs` — `context_green()`, `context_yellow()`, `context_red()` color functions (respect `ColorMode`)

---

## External Editor Mode

**Related:** [PRD §16.8](../../planning/PRD.md#168-external-editor-mode)

Allows users to compose prompts in a full-featured external editor instead of the built-in input widget.

### Module

New file: `tui/src/editor.rs`

### Flow

```
User types /editor
    │
    ├── Create temp file in std::env::temp_dir()
    ├── Leave alternate screen (crossterm)
    ├── Disable raw mode
    ├── Spawn $EDITOR <tempfile> as child process
    ├── Wait for editor to exit
    ├── Re-enable raw mode
    ├── Re-enter alternate screen
    ├── Re-initialize crossterm::EventStream
    │
    ├── Editor exit code == 0 AND file non-empty
    │   └── Submit file contents as user prompt
    │
    └── Editor exit code != 0 OR file empty
        └── Treat as cancellation, show system message
```

### Editor Resolution

1. `editor_command` from `TuiConfig` (if set)
2. `$EDITOR` environment variable
3. `$VISUAL` environment variable
4. `vi` (fallback)

### Terminal Handoff

Uses the same `restore_terminal()` / `setup_terminal()` helpers from `main.rs`. Critical: `crossterm::EventStream` must be re-initialized after returning from the editor, since the old stream's file descriptor state is stale.

---

## Plan Mode

**Related:** [PRD §16.9](../../planning/PRD.md#169-plan-mode)

A read-only operating mode where the agent produces a structured plan without executing write or destructive tools.

### State

New enum on `App`:

```rust
enum OperatingMode {
    Plan,
    Execute,  // default
}
```

### Tool Filtering

When entering plan mode:
- `App::enter_plan_mode()` calls `agent.enter_plan_mode()`, which returns the saved tools and system prompt, filters to read-only tools, and appends a planning addendum to the system prompt.

When switching back to `Execute`:
- `App::exit_plan_mode()` calls `agent.exit_plan_mode(saved_tools, saved_prompt)` to restore the full tool set and original system prompt.
- The last plan message is enqueued as a follow-up message so the agent can reference it.

### UI Indicators

- Status bar shows `[PLAN]` (blue background) or `[EXEC]` (green background) badge next to the agent state indicator
- Messages produced in plan mode render with a blue left border instead of the standard cyan
- Mode toggle: `Shift+Tab` or `/plan` command, both update `operating_mode` and set `dirty = true`

---

## Collapsible Tool Result Blocks

**Related:** [PRD §16.10](../../planning/PRD.md#1610-collapsible-tool-result-blocks)

Tool invocations and their results are grouped into collapsible blocks in the conversation view, reducing visual noise during tool-heavy turns.

### Data Model Changes

New fields on `DisplayMessage` (for tool result messages):

```
collapsed: bool          — current collapse state
summary: String          — one-line summary for collapsed view
user_expanded: bool      — true if user manually expanded (prevents auto-collapse)
```

### Rendering

**Collapsed** (single line):
```
[▶] read_file  ✓  src/main.rs (42 lines)
```

**Expanded** (full content):
```
[▼] read_file  ✓  src/main.rs (42 lines)
│ fn main() {
│     let config = Config::load();
│     ...
│ }
```

### Focus Model

From any focus:
- `F2` toggles collapse/expand on the selected (or most recent) tool block
- `Shift+←` / `Shift+→` cycles the selection across tool result messages

### Auto-Collapse Behavior

- New tool results start expanded during streaming
- `tick()` checks for tool result messages that have been expanded for > 10 seconds and collapses them
- Exception: if `user_expanded == true`, the block stays expanded until the user manually collapses it

---

## Tiered Approval Modes

**Related:** [PRD §16.11](../../planning/PRD.md#1611-tiered-approval-modes)

Extends the binary on/off approval system with a risk-aware `Smart` mode.

### Core Crate Change

New variant in `ApprovalMode` enum (`src/tool.rs`):

```rust
enum ApprovalMode {
    Enabled,    // prompt for all tool calls
    Smart,      // auto-approve reads, prompt for writes (new default)
    Bypassed,   // auto-approve all tool calls
}
```

### TUI State

New field on `App`:

```
session_trusted_tools: HashSet<String>
```

Populated when the user chooses "always approve" during a Smart-mode approval prompt.

### Decision Flow

```
Tool call arrives
  │
  ├── ApprovalMode::Bypassed
  │   └── auto-approve
  │
  ├── requires_approval() == false
  │   └── auto-approve
  │
  ├── ApprovalMode::Smart
  │   ├── tool name in session_trusted_tools
  │   │   └── auto-approve
  │   └── else
  │       └── prompt: [y]es / [n]o / [a]lways
  │
  └── ApprovalMode::Enabled
      └── prompt: [y]es / [n]o
```

### Command Updates

- `#approve smart` — enable Smart mode
- `#approve on` — enable Enabled mode (prompt for all)
- `#approve off` — enable Bypassed mode (auto-approve all)
- `#approve` (no argument) — display current mode and list of session-trusted tools
