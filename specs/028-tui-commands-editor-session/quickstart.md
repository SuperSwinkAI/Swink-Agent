# Quickstart: TUI: Commands, Editor & Session

**Feature**: 028-tui-commands-editor-session | **Date**: 2026-03-20

## Prerequisites

- Rust 1.88+ (edition 2024)
- `swink-agent` and `swink-agent-memory` crates available as path dependencies
- Terminal emulator with clipboard support (macOS pasteboard, Linux X11/Wayland, Windows)
- An editor binary available (`$EDITOR`, `$VISUAL`, or `vi`)

## Build & Test

```bash
# Build the TUI crate
cargo build -p swink-agent-tui

# Run all TUI tests
cargo test -p swink-agent-tui

# Run specific component tests
cargo test -p swink-agent-tui commands     # Command parsing tests
cargo test -p swink-agent-tui editor       # External editor tests
cargo test -p swink-agent-tui session      # Session re-export tests

# Full workspace check
cargo test --workspace
cargo clippy --workspace -- -D warnings

# Launch the TUI (auto-loads .env)
cargo run -p swink-agent-tui
```

## Usage Examples

### Command Parsing

```rust
use swink_agent_tui::commands::{execute_command, CommandResult, ClipboardContent};

// Hash commands (TUI-internal)
match execute_command("#help") {
    CommandResult::ToggleHelp => println!("Show help panel"),
    _ => {}
}

match execute_command("#clear") {
    CommandResult::Clear => println!("Clear conversation"),
    _ => {}
}

match execute_command("#copy code") {
    CommandResult::CopyToClipboard(ClipboardContent::Code) => {
        println!("Copy code blocks to clipboard");
    }
    _ => {}
}

// Slash commands (agent config)
match execute_command("/quit") {
    CommandResult::Quit => println!("Exit TUI"),
    _ => {}
}

match execute_command("/system You are a pirate.") {
    CommandResult::SetSystemPrompt(prompt) => {
        println!("New prompt: {prompt}");
    }
    _ => {}
}

// Not a command
match execute_command("hello world") {
    CommandResult::NotACommand => println!("Send as message to agent"),
    _ => {}
}

// Unknown command
match execute_command("#nonexistent") {
    CommandResult::Feedback(msg) => println!("Error: {msg}"),
    _ => {}
}
```

### External Editor

```rust
use swink_agent_tui::editor::{resolve_editor, open_editor};

// Resolve editor binary
let editor = resolve_editor(Some("nano"));  // config override wins
let editor = resolve_editor(None);          // checks $EDITOR, $VISUAL, then vi

// Open editor and capture result
match open_editor(&editor) {
    Ok(Some(content)) => println!("User wrote: {content}"),
    Ok(None) => println!("User cancelled (empty file)"),
    Err(e) => eprintln!("Editor error: {e}"),
}
```

### Session Persistence

```rust
use swink_agent_tui::session::{JsonlSessionStore, SessionMeta, SessionStore};
use std::path::PathBuf;

// Create a session store
let store = JsonlSessionStore::new(PathBuf::from("/tmp/sessions")).unwrap();

// Generate a session ID
let id = store.new_session_id();

// Save a session (messages come from the agent)
store.save(&id, "claude-sonnet-4-20250514", "You are helpful.", &messages).unwrap();

// List saved sessions
let sessions = store.list().unwrap();
for meta in &sessions {
    println!("{}: {} ({} messages)", meta.id, meta.model, meta.message_count);
}

// Load a session
let (meta, messages) = store.load(&id).unwrap();
println!("Loaded session {} with {} messages", meta.id, messages.len());
```

## TUI Commands Reference

### Hash Commands (In-Session)

| Command | Action |
|---------|--------|
| `#help` | Toggle help panel |
| `#clear` | Clear conversation display |
| `#info` | Show session info (model, messages, tokens) |
| `#copy` | Copy last assistant message to clipboard |
| `#copy all` | Copy entire conversation to clipboard |
| `#copy code` | Copy code blocks from last response to clipboard |
| `#approve on/off/smart` | Set tool approval mode |
| `#approve` | Query current approval mode |
| `#save` | Save current session |
| `#load <id>` | Load a saved session |
| `#sessions` | List saved sessions |
| `#key <provider> <key>` | Store an API key |
| `#keys` | List configured credentials |

### Slash Commands (Agent/App)

| Command | Action |
|---------|--------|
| `/quit` or `/q` | Exit the TUI |
| `/thinking <level>` | Set thinking level (off/low/medium/high) |
| `/system <prompt>` | Set system prompt |
| `/reset` | Reset conversation and agent state |
| `/editor` | Open external editor for prompt composition |
| `/plan` | Toggle plan mode |

## Key Files

| File | Purpose |
|------|---------|
| `tui/src/commands.rs` | `execute_command`: hash/slash command parsing, `CommandResult` enum |
| `tui/src/editor.rs` | `resolve_editor`, `open_editor`: external editor integration |
| `tui/src/session.rs` | Re-exports `SessionStore` and `JsonlSessionStore` from memory crate |
| `tui/src/app/state.rs` | App state: session ID, clipboard bridge, command result handling |
| `tui/src/app/event_loop.rs` | Routes `CommandResult` variants to app actions |
| `tui/src/lib.rs` | Crate root: re-exports, `setup_terminal`, `restore_terminal` |
