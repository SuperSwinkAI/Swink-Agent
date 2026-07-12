# Data Model: TUI: Commands, Editor & Session

**Feature**: 028-tui-commands-editor-session | **Date**: 2026-03-20

## Entity: CommandResult (enum, public)

**Location**: `tui/src/commands.rs`

| Variant | Payload | Purpose |
|---------|---------|---------|
| `Feedback(String)` | Feedback text | Command produced text to display in conversation |
| `Quit` | — | Request TUI exit |
| `Clear` | — | Clear conversation display |
| `SetThinking(String)` | Level string | Change thinking level (off/minimal/low/medium/high/extra-high) |
| `SetSystemPrompt(String)` | Prompt text | Update system prompt |
| `Reset` | — | Reset conversation and agent state |
| `CopyToClipboard(ClipboardContent)` | Content selector | Copy specified content to clipboard |
| `SaveSession` | — | Save current session |
| `LoadSession(String)` | Session ID | Load a saved session |
| `ListSessions` | — | List all saved sessions |
| `StoreKey { provider, key }` | Provider + API key | Store a credential |
| `ListKeys` | — | List configured credentials |
| `SetApprovalMode(ApprovalModeArg)` | Mode | Set tool approval mode |
| `QueryApprovalMode` | — | Query current approval mode |
| `OpenEditor` | — | Open external editor |
| `TogglePlanMode` | — | Toggle plan mode |
| `ToggleHelp` | — | Toggle help panel |
| `NotACommand` | — | Input was not a command |

**Derives**: `Debug`

---

## Entity: ClipboardContent (enum, public)

**Location**: `tui/src/commands.rs`

| Variant | Purpose |
|---------|---------|
| `Last` | Copy last assistant message |
| `All` | Copy entire conversation |
| `Code` | Copy code blocks from last assistant response |

**Derives**: `Debug`, `Clone`, `Copy`

---

## Entity: ApprovalModeArg (enum, public)

**Location**: `tui/src/commands.rs`

| Variant | Purpose |
|---------|---------|
| `On` | Enable tool approval for all tools |
| `Off` | Disable tool approval |
| `Smart` | Selective approval (dangerous tools only) |

**Derives**: `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`

---

## Entity: ExternalEditor (logical, module-level functions)

**Location**: `tui/src/editor.rs`

Not a struct. Editor integration is a pair of stateless functions:

| Function | Signature | Purpose |
|----------|-----------|---------|
| `resolve_editor()` | `fn(config_override: Option<&str>) -> String` | Resolve editor binary: config > `$EDITOR` > `$VISUAL` > `vi` |
| `open_editor()` | `fn(editor_command: &str) -> io::Result<Option<String>>` | Open editor with temp file; return content or `None` on cancel |

**Behavior**:
- `resolve_editor` checks sources in priority order, returning the first non-empty value
- `open_editor` creates a temp file via `tempfile::NamedTempFile::new()?.into_temp_path()` [corrected 2026-07-06: not a deterministic `swink-prompt-{pid}.md` name — `NamedTempFile` generates a randomized, collision-resistant filename in the system temp directory], launches the editor, reads the file on close, and deletes it
- Empty/whitespace-only file after editor close = cancellation (`Ok(None)`)
- Non-zero exit status or missing binary = `Err(io::Error)`
- Temp file cleaned up in all code paths (success, error, cancellation)

---

## Entity: SessionPersistence (logical, re-export + app integration)

**Location**: `tui/src/session.rs` (re-exports), `tui/src/app/state.rs` (usage)

The TUI's session module re-exports from `swink-agent-memory`:

| Re-export | Type | Purpose |
|-----------|------|---------|
| `SessionStore` | Trait | Pluggable persistence interface |
| `JsonlSessionStore` | Struct | JSONL-based file persistence |

The app holds session state:

| Field | Type | Purpose |
|-------|------|---------|
| `session_id` | `Option<String>` | Current session ID (set on save or load) |
| `session_store` | `Option<JsonlSessionStore>` | Persistence backend (initialized from config) |

**Session save flow** [corrected 2026-07-06: the app uses `save_full`, not the simpler `save`, so it can atomically persist a crash-recovery state snapshot alongside the transcript]:
1. App generates or reuses `session_id`
2. Builds a `SessionMeta` (preserving `created_at`/`sequence` from the prior save when updating) and takes a snapshot of the agent's `SessionState`
3. Calls `store.save_full(id, &meta, &messages, &state_snapshot)`, which returns the persisted `SessionMeta` (with bumped `sequence`)
4. `JsonlSessionStore` writes line 1 = `SessionMeta` JSON, line 2 = state snapshot JSON, lines 3+ = one `AgentMessage` JSON per line
5. Feedback message confirms save

**Session load flow** [corrected 2026-07-06: the app uses `load_full`, which also returns the crash-recovery snapshot]:
1. App calls `store.load_full(id, registry)` which returns `(SessionMeta, Vec<AgentMessage>, Option<serde_json::Value>)`
2. If a state snapshot is present, it is restored via `SessionState::restore_from_snapshot`; otherwise a fresh `SessionState::new()` is used
3. Messages replayed into conversation view as `DisplayMessage` entries
4. Session title restored from `SessionMeta`
5. `session_id` set to loaded ID for subsequent saves

---

## Entity: ClipboardBridge (logical, inline in event loop)

**Location**: `tui/src/app/event_loop.rs` (implemented inline in `copy_to_clipboard` method, not as a separate struct)

Clipboard operations use `arboard::Clipboard`:

| Operation | Input | Clipboard Content |
|-----------|-------|-------------------|
| `#copy` | `&[DisplayMessage]` | Last assistant message `.content` |
| `#copy all` | `&[DisplayMessage]` | All messages formatted as `Role: content\n\n` |
| `#copy code` | `&[DisplayMessage]` | Code blocks extracted from last assistant message via regex for fenced blocks |

**Code block extraction**: Scan the last assistant message's `content` for fenced code blocks (` ```...``` `). Extract the content between fences, concatenate with double newlines. If no code blocks found, return feedback "No code blocks to copy."

**Error handling**: `arboard::Clipboard::new()` or `set_text()` failure produces a user-friendly feedback message (e.g., "Clipboard unavailable"). No panic.

---

## Relationship Diagram

```text
App (app/state.rs)
  │
  ├── execute_command(input) ──► CommandResult (commands.rs)
  │     ├── #help ──► ToggleHelp
  │     ├── #clear ──► Clear
  │     ├── #info ──► Feedback(session info)
  │     ├── #copy / #copy all / #copy code ──► CopyToClipboard(variant)
  │     ├── #approve on/off/smart ──► SetApprovalMode(arg)
  │     ├── #save ──► SaveSession
  │     ├── #load <id> ──► LoadSession(id)
  │     ├── #sessions ──► ListSessions
  │     ├── /quit ──► Quit
  │     ├── /thinking <level> ──► SetThinking(level)
  │     ├── /system <prompt> ──► SetSystemPrompt(prompt)
  │     ├── /reset ──► Reset
  │     ├── /editor ──► OpenEditor
  │     ├── /plan ──► TogglePlanMode
  │     └── unrecognized ──► Feedback(error + help text)
  │
  ├── resolve_editor() / open_editor() (editor.rs)
  │     ├── config override ──► editor binary
  │     ├── $EDITOR ──► editor binary
  │     ├── $VISUAL ──► editor binary
  │     └── fallback ──► "vi"
  │
  ├── SessionStore (session.rs ──► swink-agent-memory)
  │     ├── save_full(id, meta, messages, state_snapshot) ──► persisted SessionMeta + JSONL file
  │     ├── load_full(id, registry) ──► (SessionMeta, Vec<AgentMessage>, Option<state_snapshot>)
  │     ├── list() ──► Vec<SessionMeta>
  │     └── delete(id)
  │
  └── ClipboardBridge (arboard)
        ├── copy_text(text) ──► clipboard
        └── error ──► Feedback(user-friendly message)
```
