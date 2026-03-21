# Public API Contract: TUI: Commands, Editor & Session

**Feature**: 028-tui-commands-editor-session | **Date**: 2026-03-20

## Module: `swink_agent_tui::commands`

```rust
/// Result of parsing and executing a command.
#[derive(Debug)]
pub enum CommandResult {
    Feedback(String),
    Quit,
    Clear,
    SetThinking(String),
    SetSystemPrompt(String),
    Reset,
    CopyToClipboard(ClipboardContent),
    SaveSession,
    LoadSession(String),
    ListSessions,
    StoreKey { provider: String, key: String },
    ListKeys,
    SetApprovalMode(ApprovalModeArg),
    QueryApprovalMode,
    OpenEditor,
    TogglePlanMode,
    ToggleHelp,
    NotACommand,
}

/// What to copy to clipboard.
#[derive(Debug, Clone, Copy)]
pub enum ClipboardContent {
    Last,
    All,
    Code,
}

/// Parsed approval mode argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalModeArg {
    On,
    Off,
    Smart,
}

/// Parse and execute a command string.
///
/// Returns `CommandResult` indicating what action to take.
/// Input is trimmed before parsing.
pub fn execute_command(input: &str) -> CommandResult;
```

**Contract**:

### Hash Command Parsing
- Input starting with `#` (after trimming) is parsed as a hash command.
- The `#` prefix is stripped; remaining text is trimmed and matched exactly.
- Recognized commands: `help`, `clear`, `info`, `copy`, `copy all`, `copy code`, `sessions`, `save`, `approve`, `approve on`, `approve off`, `approve smart`, `keys`.
- `load <id>` requires a non-empty ID argument; missing ID returns usage feedback.
- `key <provider> <api-key>` requires both arguments; missing key returns usage feedback.
- `approve` without argument returns `QueryApprovalMode`; invalid argument returns usage feedback.
- Unrecognized hash commands return `Feedback` with the unknown command name and a hint to type `#help`.

### Slash Command Parsing
- Input starting with `/` (after trimming) is parsed as a slash command.
- The `/` prefix is stripped; the command name is split from arguments at the first space.
- Recognized commands: `quit` (alias `q`), `thinking`, `system`, `reset`, `editor`, `plan`.
- `/thinking` requires an argument (off/low/medium/high); missing argument returns usage feedback.
- `/system` requires an argument (the new prompt); missing argument returns usage feedback.
- Unrecognized slash commands return `Feedback` with the unknown command name and a hint to type `#help`.

### Not-a-Command
- Input not starting with `#` or `/` (after trimming) returns `NotACommand`.
- Empty and whitespace-only input returns `NotACommand`.

---

## Module: `swink_agent_tui::editor`

```rust
/// Resolve the editor command from environment or fallback.
///
/// Priority: config override > `$EDITOR` > `$VISUAL` > `vi`.
#[must_use]
pub fn resolve_editor(config_override: Option<&str>) -> String;

/// Open the editor with a temporary file and return the file contents on close.
///
/// Returns `Ok(Some(content))` if the editor exited successfully and the file
/// is non-empty (trimmed).
/// Returns `Ok(None)` if the editor exited successfully but the file is empty
/// or whitespace-only (cancellation).
/// Returns `Err` if the editor could not be launched or exited with a non-zero
/// status.
pub fn open_editor(editor_command: &str) -> io::Result<Option<String>>;
```

**Contract**:

### Editor Resolution
- If `config_override` is `Some(s)`, return `s` regardless of environment variables.
- If `config_override` is `None`, check `$EDITOR` environment variable. If set and non-empty, return it.
- If `$EDITOR` is unset or empty, check `$VISUAL`. If set and non-empty, return it.
- If neither is set, return `"vi"`.

### Editor Open
- Creates a temporary file at `{std::env::temp_dir()}/swink-prompt-{pid}.md`.
- Launches `editor_command` with the temp file path as the sole argument.
- Blocks until the editor process exits.
- On successful exit (status 0): reads the temp file, trims content.
  - Non-empty trimmed content: returns `Ok(Some(content))`.
  - Empty/whitespace trimmed content: returns `Ok(None)` (cancellation).
- On failed exit (non-zero status): returns `Err` with status in message.
- On launch failure (binary not found): returns `Err` with the underlying OS error.
- The temp file is deleted in all code paths (success, error, cancellation).

---

## Module: `swink_agent_tui::session`

```rust
// Re-exports from swink-agent-memory
pub use swink_agent_memory::{JsonlSessionStore, SessionStore};
```

**Contract**:

### Re-exports
- `SessionStore`: the persistence trait with `save`, `load`, `list`, `delete`, `new_session_id`.
- `JsonlSessionStore`: the JSONL-based file persistence implementation.
- See `swink-agent-memory` crate documentation for full trait contract.

### TUI Integration (in app)
- `#save` triggers `store.save(session_id, model, system_prompt, &agent_messages)`.
- `#load <id>` triggers `store.load(id)` and replaces conversation state.
- `#sessions` triggers `store.list()` and displays session metadata as feedback.
- Session ID is generated via `store.new_session_id()` on first save if not already set.
- Corrupted or missing session files produce an error feedback message; the TUI continues with its current state.

---

## Clipboard Integration (in app event loop)

```rust
// Not a public API — internal to app event loop handling of CopyToClipboard
```

**Contract**:

### Copy Operations
- `ClipboardContent::Last`: extract `content` from the last `DisplayMessage` with `role == Assistant`. If none exists, show "Nothing to copy" feedback.
- `ClipboardContent::All`: format all messages as `"{role}: {content}\n\n"` and concatenate. Copy the full text.
- `ClipboardContent::Code`: scan the last assistant message's `content` for fenced code blocks (lines between ` ``` ` delimiters). Extract inner content, concatenate with double newlines. If no code blocks found, show "No code blocks to copy" feedback.
- On successful copy, show brief confirmation feedback (e.g., "Copied to clipboard").
- On clipboard error (e.g., no clipboard available), show informative error feedback. No panic.
