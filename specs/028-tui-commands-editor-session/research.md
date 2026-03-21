# Research: TUI: Commands, Editor & Session

**Feature**: 028-tui-commands-editor-session | **Date**: 2026-03-20

## Decision 1: Command Parsing — Simple String Matching

**Question**: How should hash and slash commands be parsed and dispatched?

**Decision**: A single `execute_command` function that `trim()`s input, checks for `#` or `/` prefix, then dispatches via `match` on the remaining string. Returns a `CommandResult` enum that the event loop interprets.

**Rationale**: The command set is small and fixed. Commands are exact strings (after trimming), not a grammar. A match-based dispatcher is ~150 lines, fully testable, and has zero runtime cost beyond string comparison. The `CommandResult` enum decouples parsing from execution — the event loop handles side effects (clearing conversation, quitting, copying to clipboard), keeping `commands.rs` pure and testable without mocking terminal or agent state.

**Alternatives rejected**:
- *Trait-based command registry*: Over-engineered for ~15 commands. Adds indirection without extensibility benefit — commands are not user-defined.
- *Regex-based parsing*: Unnecessary complexity for prefix + exact-match semantics. Adds a dependency for no gain.

## Decision 2: External Editor Integration — Blocking Process with Temp File

**Question**: How should the external editor be launched and its result captured?

**Decision**: `resolve_editor` checks config override, then `$EDITOR`, then `$VISUAL`, then falls back to `vi`. `open_editor` creates a temp file in `std::env::temp_dir()`, launches the editor as a blocking `std::process::Command`, reads the file on close, and cleans up. Empty file = cancellation (`Ok(None)`). Non-zero exit or missing binary = `Err`.

**Rationale**: The editor is a blocking, synchronous process. The TUI must suspend its rendering while the editor is open (it takes over the terminal). `std::process::Command::status()` blocks until the editor exits, which is exactly the required behavior. The temp file approach is standard (git commit messages work the same way). Cleanup happens in all paths (success, error, cancellation) via explicit `remove_file` calls.

**Alternatives rejected**:
- *Async process spawning*: The editor takes over stdout/stdin. The TUI cannot render while it's running. Async adds complexity with no benefit since we must wait for completion.
- *Piped I/O*: Editors are interactive — they need direct terminal access. Piping breaks terminal editors like vim/nano.

## Decision 3: Session Persistence — Delegate to Memory Crate's JsonlSessionStore

**Question**: How should conversation sessions be saved and loaded?

**Decision**: The TUI's `session.rs` re-exports `SessionStore` and `JsonlSessionStore` from `swink-agent-memory`. The app holds an `Option<Box<dyn SessionStore>>` initialized from the config's sessions directory. Save writes the current `AgentMessage` history via `store.save()`. Load reads messages via `store.load()` and replays them into the conversation view.

**Rationale**: The memory crate already provides a complete, tested session persistence implementation using JSONL. The store writes one JSON line per message (streaming, no full buffering), handles metadata (model, system prompt, timestamps, message count), and supports listing/filtering sessions. Re-using this avoids duplicating persistence logic in the TUI crate and maintains the dependency chain (TUI depends on memory crate). The `SessionStore` trait allows swapping backends without TUI changes.

**Alternatives rejected**:
- *SQLite in TUI crate*: Would duplicate the storage concern that memory crate owns. Violates Constitution I (Library-First) boundary.
- *Custom binary format*: JSONL is human-readable, appendable, and already implemented. No reason to diverge.

## Decision 4: Clipboard Abstraction — arboard with ClipboardBridge Wrapper

**Question**: How should clipboard operations work across platforms?

**Decision**: Use the `arboard` crate for cross-platform clipboard access. Wrap it in a `ClipboardBridge` struct that provides `copy_text(&self, text: &str) -> Result<(), String>`. The bridge catches platform errors and returns user-friendly messages. Hash command handlers (`#copy`, `#copy all`, `#copy code`) extract the appropriate text and pass it to the bridge.

**Rationale**: `arboard` is the most widely used Rust clipboard crate. It supports macOS (pasteboard), Linux (X11/Wayland via `wl-copy`/`xclip`), and Windows. The bridge pattern isolates the platform dependency so tests can verify text extraction logic without requiring a clipboard. The `ClipboardContent` enum (`Last`, `All`, `Code`) in `CommandResult` tells the event loop what to extract; the bridge handles the platform call.

**Alternatives rejected**:
- *cli-clipboard*: Less maintained than arboard, fewer platform options.
- *Direct pasteboard FFI*: Violates `#[forbid(unsafe_code)]` and only works on one platform.
- *No abstraction (inline arboard calls)*: Makes testing harder and scatters platform-specific error handling.
