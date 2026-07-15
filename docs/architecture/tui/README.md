# TUI Architecture

**Related Documents:**
- [PRD](../../planning/PRD.md) §16
- [HLD](../HLD.md)
- [TUI Implementation Phases](../../planning/TUI_PHASES.md)

---

## Overview

The TUI (`swink-agent-tui`) is a separate binary crate that provides an interactive terminal interface for the swink agent. It renders streaming conversations, tool executions, and agent state in a full-screen terminal application.

The implementation uses `ratatui` for rendering and `crossterm` for terminal I/O, following an immediate-mode rendering pattern where the entire UI is redrawn each frame from current state. The async event loop uses `crossterm::EventStream` with `tokio::select!` and a dirty flag to avoid unnecessary redraws.

The TUI selects a provider by priority: Custom SSE Proxy > OpenAI > Anthropic > Local (SmolLM3) > Ollama (fallback). Provider selection is driven by environment variables and keychain credentials. See [getting_started.md](../../getting_started.md) for the full priority table.

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
│   │   └── Inline diff rendering (for file modifications)
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
├── app/           — App state, async event loop, key handling, agent dispatch
│   ├── mod.rs           — Re-exports and App struct definition
│   ├── agent_bridge.rs  — Agent ↔ TUI event bridging
│   ├── event_loop.rs    — Async event loop (tokio::select! multiplexer)
│   ├── lifecycle.rs     — App startup and shutdown logic
│   ├── persistence.rs   — Session save/load integration
│   ├── render_helpers.rs — Shared rendering utilities
│   └── state.rs         — App state fields and mutations
├── commands.rs    — Command parsing: hash commands (#…) and slash commands
│                    (/…). Full list in §Command System below
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
    └── diff.rs          — DiffData + render_diff_lines(): unified and
                           side-by-side diff rendering for file modifications
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
| `Escape` | Clear active chat selection; otherwise abort running agent |
| `Ctrl+C` | Copy active chat selection; otherwise abort agent or quit if idle |
| `Ctrl+Q` | Quit application |
| `Up/Down` | Scroll conversation (conversation focus) / input history (input focus) |
| `Page Up/Down` | Scroll conversation by page |
| `Mouse wheel` | Scroll conversation (clears any active selection) |
| `Click + drag` (conversation) | Anchor/extend in-app text selection; release copies to clipboard |
| `Home` / `Ctrl+A` | Move cursor to start of line |
| `End` / `Ctrl+E` | Move cursor to end of line |
| `Tab` | Cycle focus between Input and Conversation |
| `Shift+Tab` | Toggle between Plan and Execute mode |
| `F1` | Toggle help side panel |
| `F2` | Expand/collapse selected tool result block |
| `F3` | Cycle color mode (Custom → MonoWhite → MonoBlack) |
| `F4` | Cycle model (applied on next send) |
| `Shift+←` / `Shift+→` | Select previous/next tool block |
| `h` (at a `write_file` approval prompt) | Open per-hunk review of the pending diff |
| `y` / `n` / `a` (in hunk review) | Apply hunk / revert hunk / apply all remaining hunks |
| `Escape` (in hunk review) | Cancel the review and return to the whole-call approval prompt |

Typing any printable character auto-focuses the input editor.

---

## Focus Management

Tab cycles focus between the Input Editor and Conversation View. The focused component renders with a brighter border to indicate selection. Typing any character automatically shifts focus to the input editor.

---

## Rendering Pipeline

1. **State update** — Terminal or agent events mutate `App` state and set the dirty flag
2. **Layout** — `ratatui::Layout` computes widget areas from terminal dimensions
3. **Render** — Each component renders into its allocated `Rect`:
   - Conversation view: iterates messages, renders each with role-colored left border and markdown-formatted content; after the `Paragraph` draws, a post-render pass captures per-row cell symbols (for selection copy) and applies `Modifier::REVERSED` to any cells inside the active selection range
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
- `#approve smart` — enable smart approval mode (auto-approve read-only and trusted tools, prompt for writes)
- `#approve on` / `#approve off` — enable Enabled / Bypassed mode
- `#approve untrust <tool>` — remove a tool from session-trusted set
- `#approve untrust` — clear all session-trusted tools
- `#approve` — display current mode and trusted tools

**Slash commands** (may affect agent state):
- `/quit` — exit the application
- `/thinking` — toggle thinking display
- `/system` — set the system prompt
- `/reset` — reset the conversation
- `/editor` — open external editor for prompt composition
- `/plan` — toggle plan mode (read-only analysis)

Clipboard operations use the `arboard` crate — both hash-command copies (`#copy`, `#copy all`, `#copy code`) and in-app click-drag selection share the same bridge. When the clipboard is unavailable, the failure surfaces as a non-fatal system message.

---

## Chat Text Selection

Because the TUI owns mouse capture for scroll-wheel support, the terminal emulator cannot do its own click-drag selection. The TUI provides an in-app equivalent:

- `MouseEventKind::Down(Left)` inside the conversation viewport anchors a `Selection` at the pointer cell.
- `Drag(Left)` extends `Selection::cursor` (clamped to the viewport edges so the highlight keeps extending when the pointer leaves the area).
- `Up(Left)` writes the selected text to the system clipboard via `arboard` and leaves the highlight visible until explicitly cleared.
- `Ctrl+C` with an active selection copies and clears; `Esc` clears without copying; starting a new click-drag or rolling the scroll wheel also clears the existing selection.

Coordinates are `(row, col)` inside the conversation's inner area (after the block border). Copy text is extracted from per-row cell symbols captured during the same render pass that applied the highlight, so the clipboard reflects exactly what was on screen (wrapping included). The role-colored gutter (`│ `) is stripped per line and trailing whitespace is trimmed.

Terminal-native bypasses continue to work unchanged on terminals that support them: Shift-drag on kitty / Alacritty / WezTerm / Ghostty, Option-drag on iTerm2, Fn-drag on Terminal.app.

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
| `system_prompt` | `Option<String>` | Override the system prompt passed to the agent. If `None`, the agent uses its built-in default. |
| `pricing` | `PricingTable` | Operator-declared per-model rates. Empty by default. See [Cost and usage display](#cost-and-usage-display). |

---

## Cost and usage display

The status bar renders `↓{input} ↑{output}` and `${cost}`; `/usage` prints a
per-turn breakdown with per-model subtotals. Both read `App::total_input_tokens`
/ `total_output_tokens` / `total_cost` / `turn_usage`, which are accumulated in
`App::handle_agent_event` from `AgentEvent::MessageEnd`.

**The TUI does not price anything.** The agent loop fills in each assistant
message's `Cost` before emitting `MessageEnd` — see
[Cost tracking](../streaming/README.md#cost-tracking) for the precedence rules.
The TUI only totals what it is given, so a model with no pricing honestly shows
`$0.0000` (and `/usage` says which models those are).

Because the compiled model catalog only knows about models shipped with the
crate, operators can declare their own rates for local endpoints, private
deployments, or negotiated per-tier pricing:

```toml
[pricing."my-local-llama"]
input_per_million = 0.10
output_per_million = 0.40

[pricing."claude-sonnet-4-6"]
input_per_million = 1.50   # negotiated below the catalog's $3.00
output_per_million = 7.50
```

Declared rates take precedence over the catalog for any model listed.
`launch` / `launch_with_extensions` / `launch_with_session` apply them via
`TuiConfig::apply_pricing`; a host that builds its own `Agent` calls that (or
`AgentOptions::with_cost_calculator`) directly.

---

## Host extension points

`TuiConfig` is deserialized from TOML and so can only hold data. Anything a host
supplies *in code* goes on `TuiExtensions`, passed via `App::with_extensions` or
`launch_with_extensions`:

```rust,ignore
let extensions = TuiExtensions::new().with_command("spend", |app, _args| {
    CustomCommandOutcome::Feedback(format!("${:.4}", app.total_cost))
});
launch_with_extensions(config, &mut terminal, options, extensions).await?;
```

Host commands are matched by bare name (no sigil, so `/spend` and `#spend` both
route) **before** the built-in command table, so a host can shadow a built-in;
returning `CustomCommandOutcome::NotHandled` falls through to it instead. Secret
classification (`#key`) runs before dispatch, so host handlers never see
credentials.

`TuiExtensions` is a consuming builder with a `Default` impl — further seams are
added as additional `with_*` methods without breaking existing callers.

### `@path` file mentions

Two seams, deliberately split by *when* they run:

```rust,ignore
let extensions = TuiExtensions::new()
    // 1. Discovery — runs per keystroke inside an `@` mention.
    .with_path_completions(|query| my_index.matching(query).map(PathCandidate::new).collect())
    // 2. Expansion — runs once per submitted prompt that contains a mention.
    .with_mention_resolver(|text, mentions| {
        let mut out = text.to_string();
        for mention in mentions.iter().rev() {   // back-to-front keeps spans valid
            let body = std::fs::read_to_string(&mention.path).ok()?;
            out.replace_range(mention.start..mention.end, &body);
        }
        Some(out)
    });
```

The TUI never touches the filesystem. It parses mentions (`parse_mentions`),
renders the popup, and hands the host `PathMention`s with byte spans; the host
owns path discovery, working-directory semantics, ignore rules, and file reads.

**Resolution is lazy by construction.** The resolver is called from
`send_to_agent`, not from the input handler, so:

- typing `@src/lib.rs` reads nothing — only the completion provider runs;
- the resolver runs once, at submit, and only when the text holds a mention;
- the conversation view keeps showing the raw `@src/lib.rs`, while the agent
  receives the expansion. `mentions_resolve_at_submit_and_never_while_typing`
  pins this.

A mention is an `@` that starts the text or follows whitespace, plus the
following non-whitespace run, minus trailing sentence punctuation — so
`user@example.com` is not a mention and `see @src/lib.rs.` mentions
`src/lib.rs`. While the popup is open it takes Up/Down (navigate), Tab/Enter
(accept), and Esc (dismiss); each falls through to its normal binding when the
popup is closed.

### Skills

Three seams, one per tier of progressive disclosure:

```rust,ignore
let extensions = TuiExtensions::new()
    // Tier 1 — candidates, per keystroke inside a leading `/name`.
    .with_skill_completions(|query| {
        my_index.matching(query)
            .map(|s| SkillCandidate::new(&s.name).with_description(&s.summary))
            .collect()
    })
    // Tier 2 — SKILL.md body, on highlight (cached per popup).
    .with_skill_details(|name| my_index.body_of(name))
    // Tier 3 — expansion, once per submitted prompt that starts with `/name`.
    .with_skill_resolver(|text, invocation| {
        let body = my_index.body_of(&invocation.name)?;
        let mut out = text.to_string();
        out.replace_range(invocation.start..invocation.end, &body);
        Some(out)
    });
```

Unlike a mention, an invocation is **leading-only**: the `/` must be the first
non-whitespace character of the prompt (`parse_skill_invocation`), matching the
command table's single-leading-sigil model — `either/or` and `/usr/bin`
mid-sentence never trigger anything. The popup itself mirrors the `@path` one
(same keys, at most one of the two popups open at a time), with the highlighted
skill's tier-2 documentation rendered as a clamped preview below the list.

Submit-time dispatch precedence is secrets → host commands → skills →
built-ins, first match wins: a known skill (exact name match against the
completion provider) is submitted as a prompt instead of falling to the
Unknown-command feedback, and a host `with_command` of the same name shadows
it. As with mentions, resolution is lazy and the transcript keeps showing the
raw `/deploy` while the agent receives the expansion. Mentions are expanded
*before* the skill invocation, on the raw text, so a skill body is never
mention-scanned — a SKILL.md containing `@/etc/passwd` cannot induce a host
file read. `skill_body_is_read_at_submit_and_never_while_typing` and
`a_skill_body_is_never_mention_scanned` pin this.

For hosts without their own index, the off-by-default `skills` cargo feature
adds `TuiExtensions::with_skill_dirs`, which eagerly indexes
`<dir>/<name>/SKILL.md` (YAML frontmatter: `name`, `description`) under
*explicitly passed* directories only — there are no implicit default paths —
and wires all three seams over that index.

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

Versions live in `tui/Cargo.toml`.

| Crate | Purpose |
|---|---|
| `swink-agent` | Core agent library |
| `swink-agent-adapters` | LLM provider adapters (Ollama, proxy) |
| `swink-agent-memory` | Session persistence, compaction strategies |
| `ratatui` / `crossterm` | Terminal UI framework and backend (`event-stream` feature) |
| `tokio` / `futures` | Async runtime and stream utilities |
| `syntect` | Syntax highlighting for code blocks |
| `arboard` | Clipboard access |
| `toml` / `serde` / `dirs` | Configuration parsing and platform config dirs |
| `keyring` | Cross-platform keychain integration for API key storage |
| `tracing` / `tracing-subscriber` / `tracing-appender` | File-based logging (daily rolling) |

---

## Inline Diff View

**Related:** [PRD §16.6](../../planning/PRD.md#166-inline-diff-view)

When a tool result's details carry write-file diff fields (path, new-file flag, old/new content — emitted by `WriteFileTool`), the `ToolExecutionEnd` handler parses them into a `DiffData` (`tui/src/ui/diff.rs`) attached to the tool-result message, and the conversation view renders a syntax-highlighted diff instead of raw tool output:

- **Unified** (default): `+`/`-` prefixed lines with green/red styling
- **Side-by-side**: two columns (old | new) when the terminal is ≥ 160 columns wide and the file is not newly created; falls back to unified otherwise
- Output is truncated after 50 diff lines

### Per-Hunk Approve/Reject

Diffs rendered from a *tool result* are display-only — the write has already happened. Per-hunk review instead runs at the **approval prompt**, before the write is applied:

- `WriteFileTool::approval_context()` reads the file currently on disk and returns the same `path` / `is_new_file` / `old_content` / `new_content` shape as its result `details`, so `DiffData` parses both. It returns `None` (and the TUI falls back to the plain prompt) when the path cannot be safely resolved inside the execution root.
- At a `write_file` approval prompt, `h` opens the review. `compute_hunks()` splits the change into maximal runs of non-common lines; each hunk is one `y` (apply) / `n` (revert) decision, `a` applies all remaining, `Escape` cancels back to the whole-call prompt. The review panel shows one hunk at a time with an `i/n` progress header.
- On completion the pending approval is answered: all approved → `ToolApproval::Approved`; all rejected → `ToolApproval::Rejected`; mixed → `ToolApproval::ApprovedWith`, carrying content rebuilt by `merge_hunks()` so rejected hunks keep their original lines. Approving every hunk reproduces the proposed content byte-for-byte; rejecting every hunk reproduces the original.
- Rejected hunks generate a follow-up message to the agent naming which hunks were reverted, steered in at the next turn boundary so the agent does not assume its write landed intact.

Per-hunk review is offered only for modifications to existing files. New files have no original content to fall back to, so the whole-call `y`/`n` prompt already covers them.

> **Planned — not implemented**: per-hunk review inside the read-only diff blocks rendered in the conversation (post-write). Reverting an already-applied write is not supported; review happens at approval time only.

---

## Context Window Progress Bar

**Related:** [PRD §16.7](../../planning/PRD.md#167-context-window-progress-bar)

The status bar renders a 10-character gauge with a percentage label (`[████████░░] 82%`) showing context-window fill. Fill is recomputed on `TurnEnd` using the `estimate_tokens` heuristic (chars / 4) against the model's context budget. Colors respect `ColorMode` via `theme.rs`: `context_green()` < 60%, `context_yellow()` 60–85%, `context_red()` > 85%.

---

## External Editor Mode

**Related:** [PRD §16.8](../../planning/PRD.md#168-external-editor-mode)

`/editor` (`tui/src/editor.rs`) composes a prompt in an external editor: the TUI writes a temp file, restores the terminal (leave alternate screen, disable raw mode), and spawns the editor. On exit code 0 with non-empty content, the file contents are submitted as the user prompt; otherwise it is treated as cancellation. Editor resolution order: `editor_command` (TuiConfig) → `$EDITOR` → `$VISUAL` → `vi`. After returning, the `crossterm::EventStream` is re-initialized because the old stream's file descriptor state is stale.

---

## Plan Mode

**Related:** [PRD §16.9](../../planning/PRD.md#169-plan-mode)

`OperatingMode { Plan, Execute }` on `App` (default `Execute`), toggled with `Shift+Tab` or `/plan`. Entering plan mode calls `agent.enter_plan_mode()`, which saves the current tools and system prompt, filters to read-only tools, and appends a planning addendum to the system prompt. Exiting calls `agent.exit_plan_mode(saved_tools, saved_prompt)` to restore both, and enqueues the last plan message as a follow-up so the agent can reference it. The status bar shows a `[PLAN]` (blue) / `[EXEC]` (green) badge, and plan-mode messages render with a blue left border instead of the standard cyan.

---

## Collapsible Tool Result Blocks

**Related:** [PRD §16.10](../../planning/PRD.md#1610-collapsible-tool-result-blocks)

Tool result messages carry `collapsed`, `summary`, and `user_expanded` state. Collapsed blocks render as a single summary line (`[▶] read_file ✓ src/main.rs (42 lines)`); expanded blocks show full content. New tool results start expanded and are auto-collapsed by `tick()` after 10 seconds unless the user expanded them manually (`user_expanded`). `F2` toggles the selected (or most recent) tool block; `Shift+←` / `Shift+→` cycles the selection.

---

## Tiered Approval

**Related:** [PRD §16.11](../../planning/PRD.md#1611-tiered-approval-modes)

`ApprovalMode` (`src/tool.rs`) has three variants: `Enabled` (prompt for every tool call), `Smart` (the default — auto-approve read-only and session-trusted tools, prompt for writes), and `Bypassed` (auto-approve everything). In Smart mode the prompt offers `[y]es / [n]o / [a]lways`; choosing "always" adds the tool to `App::session_trusted_tools` for the rest of the session. Modes and the trusted set are managed with the `#approve` command family listed in [§Command System](#command-system).
