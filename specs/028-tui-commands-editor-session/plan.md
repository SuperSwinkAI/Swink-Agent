# Implementation Plan: TUI: Commands, Editor & Session

**Branch**: `028-tui-commands-editor-session` | **Date**: 2026-03-20 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/028-tui-commands-editor-session/spec.md`

## Summary

Implement the TUI command system (hash and slash commands), external editor integration, session persistence, and clipboard support. Hash commands (`#help`, `#clear`, `#info`, `#copy`, `#approve`) perform in-session TUI actions. Slash commands (`/quit`, `/model`, `/thinking`, `/system`, `/reset`, `/plan`, `/editor`) control agent behavior and application state. The external editor suspends the TUI, launches the user's preferred editor with a temporary file, and submits the content on close. Session persistence uses the memory crate's `SessionStore` trait with `JsonlSessionStore` for JSONL-based save/load. Clipboard operations use the `arboard` crate for cross-platform clipboard access.

## Technical Context

**Language/Version**: Rust 1.88 (edition 2024)
**Primary Dependencies**: `ratatui` 0.30, `crossterm` 0.29 (event-stream), `arboard` (clipboard), `swink-agent` (core types), `swink-agent-memory` (`SessionStore`, `JsonlSessionStore`, `SessionMeta`)
**Storage**: JSONL files via `swink-agent-memory` `JsonlSessionStore` (line 1 = `SessionMeta`, lines 2+ = `AgentMessage`)
**Testing**: `cargo test -p swink-agent-tui`; unit tests for command parsing, editor resolution, session round-trip, clipboard abstraction
**Target Platform**: Any terminal (macOS, Linux, Windows via crossterm); clipboard requires platform support
**Project Type**: Library + binary (TUI crate)
**Performance Goals**: Command parsing instant (string matching); session save streams JSONL line-by-line (no full buffering for large histories)
**Constraints**: `#[forbid(unsafe_code)]`; no provider-specific types; editor launch is blocking (`std::process::Command`)

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| # | Principle | Status | Notes |
|---|-----------|--------|-------|
| I | Library-First | PASS | `commands.rs`, `editor.rs`, `session.rs` are modules within `swink-agent-tui`. Session persistence delegates to `swink-agent-memory`'s `SessionStore` trait. No new crate needed. |
| II | Test-Driven Development | PASS | Unit tests cover command parsing (all hash/slash variants, unknown commands, whitespace handling), editor resolution (config override, env fallback, vi default), editor open/cancel, and clipboard content extraction. Session round-trip tested via `JsonlSessionStore`. |
| III | Efficiency & Performance | PASS | Command parsing is simple string matching with `trim()` and `strip_prefix()`. Session persistence uses streaming JSONL writes (line-by-line, no full buffering). Clipboard operations are fire-and-forget with feedback. |
| IV | Leverage the Ecosystem | PASS | Uses `arboard` for cross-platform clipboard access (well-maintained, widely used). Session persistence delegates to the existing `swink-agent-memory` crate's `JsonlSessionStore`. Editor resolution uses `std::process::Command`. |
| V | Provider Agnosticism | PASS | Commands, editor, session, and clipboard have no knowledge of LLM providers. Messages arrive as `AgentMessage` from core. |
| VI | Safety & Correctness | PASS | `#[forbid(unsafe_code)]` at crate root. Unrecognized commands produce feedback messages, never panics. Editor errors (missing binary, non-zero exit) produce `io::Error`. Clipboard unavailability shows informative error. Corrupted JSONL produces load error, TUI starts with empty history. |

## Project Structure

### Documentation (this feature)

```text
specs/028-tui-commands-editor-session/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── public-api.md
└── spec.md
```

### Source Code (repository root)

```text
tui/src/
├── lib.rs               # Crate root — re-exports public types
├── commands.rs          # CommandParser: hash/slash command parsing and dispatch
├── editor.rs            # ExternalEditor: resolve editor, open temp file, capture result
├── session.rs           # SessionStore re-export from swink-agent-memory
├── app/
│   ├── state.rs         # App state — holds session ID, clipboard bridge
│   └── event_loop.rs    # Routes command results to app actions
└── ui/
    └── conversation.rs  # extract_code_blocks, format_conversation for clipboard
```

**Structure Decision**: All source files already exist (`commands.rs`, `editor.rs`, `session.rs`). This feature specifies the behavior of `execute_command` (in `commands.rs`), `resolve_editor`/`open_editor` (in `editor.rs`), and the `SessionStore` re-export (in `session.rs`). Clipboard operations use `arboard::Clipboard` wrapped in a `ClipboardBridge` helper. No new files or crates are needed.

## Complexity Tracking

No constitution violations. All components fit within the existing `tui` crate boundary.
