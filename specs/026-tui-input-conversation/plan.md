# Implementation Plan: TUI: Input & Conversation

**Branch**: `026-tui-input-conversation` | **Date**: 2026-03-20 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/026-tui-input-conversation/spec.md`

## Summary

Implement the two primary interactive surfaces of the TUI: a multi-line input editor for composing messages and a scrollable conversation view for displaying agent responses. The input editor supports character editing, cursor movement, dynamic height (3-10 lines), line number gutter, Shift+Enter for newlines, Enter to submit, and Up/Down history recall. The conversation view renders messages with role-colored left borders, auto-scrolls during streaming (with manual scroll override), displays a streaming cursor indicator, and renders markdown content with syntax-highlighted code blocks. The markdown renderer handles headers, bold, italic, inline code, fenced code blocks, and bullet/numbered lists with word-wrapping. Syntax highlighting uses syntect with `OnceLock`-cached grammars.

## Technical Context

**Language/Version**: Rust latest stable (edition 2024)
**Primary Dependencies**: `ratatui` 0.30, `crossterm` 0.29 (event-stream), `syntect` 5 (syntax highlighting), `swink-agent` (core types)
**Storage**: N/A (input history is per-session, in-memory)
**Testing**: `cargo test -p swink-agent-tui`; unit tests for input editor operations, conversation scroll behavior, markdown parsing, syntax highlighting fallback
**Target Platform**: Any terminal (macOS, Linux, Windows via crossterm)
**Project Type**: Library + binary (TUI crate)
**Performance Goals**: Scrolling through 500+ messages with no perceptible lag; streaming tokens appear without buffering delay
**Constraints**: `#[forbid(unsafe_code)]`; no provider-specific types; ratatui widget model (immediate-mode rendering)

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| # | Principle | Status | Notes |
|---|-----------|--------|-------|
| I | Library-First | PASS | All components are public types in the `swink-agent-tui` crate. `InputEditor`, `ConversationView`, and markdown/syntax modules are independently testable. Re-exported from `lib.rs`. |
| II | Test-Driven Development | PASS | Unit tests cover input editor operations (insert, delete, cursor movement, history, submit), conversation scroll state (auto-scroll, disengage, re-engage, clamp), markdown parsing (inline formatting, headers, lists, code blocks, wrapping), and syntax highlighting (language recognition, fallback). |
| III | Efficiency & Performance | PASS | Syntect grammars cached via `OnceLock` (load once, zero-cost after). Conversation rendering uses indexed scroll offsets, not full traversal. Monochrome mode skips syntect entirely. Dirty-flag optimization prevents unnecessary redraws. |
| IV | Leverage the Ecosystem | PASS | Uses `ratatui` for terminal rendering, `crossterm` for input events, `syntect` for syntax highlighting. Hand-rolled markdown parser covers the LLM-output subset without pulling in a full CommonMark crate. |
| V | Provider Agnosticism | PASS | Input and conversation components have no knowledge of LLM providers. Messages arrive as `DisplayMessage` structs with role classification already resolved. |
| VI | Safety & Correctness | PASS | `#[forbid(unsafe_code)]` at crate root. Empty/whitespace submissions prevented by `trim().is_empty()` check. Scroll offsets clamped to valid ranges. Unclosed code blocks during streaming are flushed gracefully. |

## Project Structure

### Documentation (this feature)

```text
specs/026-tui-input-conversation/
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
├── ui/
│   ├── mod.rs           # UI module — re-exports sub-modules
│   ├── input.rs         # InputEditor: multi-line editor, cursor, history, rendering
│   ├── conversation.rs  # ConversationView: scroll state, role borders, streaming cursor
│   ├── markdown.rs      # markdown_to_lines: headers, inline formatting, lists, code blocks, word-wrap
│   └── syntax.rs        # highlight_code: syntect-based highlighting with OnceLock caches
├── app/
│   ├── state.rs         # DisplayMessage, MessageRole, Focus, App state
│   └── event_loop.rs    # Key event dispatch to InputEditor/ConversationView
└── theme.rs             # Color functions: user_color, assistant_color, tool_color, etc.
```

**Structure Decision**: All source files already exist. This feature specifies the behavior of `InputEditor` (in `ui/input.rs`), `ConversationView` (in `ui/conversation.rs`), `markdown_to_lines` (in `ui/markdown.rs`), and `highlight_code` (in `ui/syntax.rs`). No new files or crates are needed. The event loop in `app/event_loop.rs` dispatches key events to the appropriate component based on `Focus` state.

## Complexity Tracking

No constitution violations. All components fit within the existing `tui` crate boundary.
