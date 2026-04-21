# Implementation Plan: TUI: Scaffold, Event Loop & Config

**Branch**: `025-tui-scaffold-config` | **Date**: 2026-03-22 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/025-tui-scaffold-config/spec.md`

## Summary

Build the TUI binary crate (`swink-agent-tui`) from scratch: terminal lifecycle with panic-safe teardown, an async event loop multiplexing terminal input and agent events at 30 FPS, keyboard-driven focus management, TOML-based configuration, a color theme system with monochrome accessibility modes, OS keychain credential resolution with environment variable override, a first-run setup wizard, provider selection with prioritized fallback, terminal resize handling, minimum size warnings, and non-interactive terminal detection.

## Technical Context

**Language/Version**: Rust latest stable, edition 2024
**Primary Dependencies**: ratatui 0.30 (terminal UI framework), crossterm 0.29 (terminal control, event-stream feature), tokio (async runtime), toml 0.8 (config parsing), dirs 6 (platform-native config/data dirs), keyring 3 (OS keychain), thiserror (error types), tracing + tracing-subscriber + tracing-appender (file-based logging)
**Storage**: TOML config file at `dirs::config_dir()/swink-agent/tui.toml`; OS keychain for credentials (macOS Keychain, Windows Credential Manager, Linux secret-service)
**Testing**: `cargo test -p swink-agent-tui`, integration tests in `tui/tests/`
**Target Platform**: macOS, Linux, Windows — any terminal supporting alternate screen + raw mode
**Project Type**: Binary (TUI application) with library layer for embedding
**Performance Goals**: 30 FPS (~33ms per frame), UI responsive during agent streaming
**Constraints**: `#[forbid(unsafe_code)]`; terminal must be restored on all exit paths including panics; no provider-specific knowledge in core

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | ✅ PASS | TUI is a new workspace member crate. Core (`swink-agent`) remains free of UI deps. TUI depends on core via path dep — never the reverse. Library layer (`lib.rs`) re-exports for embedding. |
| II. Test-Driven | ✅ PASS | Unit tests for config parsing, theme resolution, credential lookup. Integration tests for terminal lifecycle. Tests written before implementation. |
| III. Efficiency & Performance | ✅ PASS | Dirty-flag rendering — only redraw when state changes. 33ms tick for 30 FPS. `tokio::select!` for non-blocking event multiplexing. |
| IV. Leverage Ecosystem | ✅ PASS | ratatui (TUI framework), crossterm (terminal backend), keyring (OS keychain), dirs (XDG dirs), toml (config) — all well-maintained, widely-used. No hand-rolled alternatives. |
| V. Provider Agnosticism | ✅ PASS | Provider selection in binary entry point (`main.rs`) only. All providers satisfy `StreamFn` trait. Core has no provider knowledge. |
| VI. Safety & Correctness | ✅ PASS | `#[forbid(unsafe_code)]`. Panic hook restores terminal. Error types via `thiserror`. No global mutable state except `ColorMode` (atomic, process-global display setting). |

**Architectural Constraints**:
- Crate count: TUI is the 7th workspace member — allowed by constitution. No additional crate needed.
- MSRV latest stable, edition 2024 — consistent with workspace.
- Concurrency: Event loop is single-threaded (`tokio::select!`). Agent operations run on separate tasks.

## Project Structure

### Documentation (this feature)

```text
specs/025-tui-scaffold-config/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
└── tasks.md             # Phase 2 output (not created by /speckit.plan)
```

### Source Code (repository root)

```text
tui/
├── Cargo.toml               # Crate manifest (workspace member)
├── AGENTS.md                # Lessons learned
├── src/
│   ├── lib.rs               # Library entry: setup_terminal(), restore_terminal(), resolve_system_prompt(), tui_approval_callback(), launch()
│   ├── main.rs              # Binary entry: panic hook, TTY check, wizard gate, provider detection, tokio runtime
│   ├── app/
│   │   ├── mod.rs           # App struct public API, re-exports
│   │   ├── state.rs         # AppState, Focus enum, AgentStatus, DisplayMessage, MessageRole, OperatingMode
│   │   ├── lifecycle.rs     # App::new(), helper methods
│   │   ├── event_loop.rs    # run() — tokio::select! loop (terminal, agent, approval, tick)
│   │   ├── agent_bridge.rs  # send_to_agent(), handle_agent_event()
│   │   └── tests.rs         # App unit tests
│   ├── ui/
│   │   ├── mod.rs           # Main render() function, layout calculation, size check overlay
│   │   ├── conversation.rs  # Scrollable conversation view (placeholder for 026)
│   │   ├── input.rs         # Input editor (placeholder for 026)
│   │   └── status_bar.rs    # Bottom status bar (placeholder for 027)
│   ├── config.rs            # TuiConfig struct, TOML loading, defaults
│   ├── credentials.rs       # ProviderInfo, credential(), store_credential(), check_credentials()
│   ├── wizard.rs            # SetupWizard — first-run provider selection + key entry
│   ├── theme.rs             # ColorMode (Custom/MonoWhite/MonoBlack), color palette functions
│   └── error.rs             # TuiError enum
└── tests/
    └── ac_tui.rs            # Integration/acceptance tests
```

**Structure Decision**: Single crate with `app/` (state + event loop) and `ui/` (rendering) module groups. The `app/` vs `ui/` split separates logic from presentation. Placeholder files for conversation, input, and status bar provide extension points for features 026–027 without premature implementation.

## Design Decisions

### D1: Event Loop Architecture

The event loop uses `tokio::select!` to multiplex four event sources without blocking:

1. **Terminal events** — crossterm `EventStream` (keyboard, mouse, resize)
2. **Agent events** — `mpsc::Receiver<AgentEvent>` from the agent runtime
3. **Approval requests** — `mpsc::Receiver<(ToolApprovalRequest, oneshot::Sender)>` for tool approval
4. **Tick** — `tokio::time::interval(33ms)` for periodic UI updates (cursor blink, animations)

A `dirty` flag gates rendering — the terminal is only redrawn when state has changed, avoiding unnecessary work during idle periods.

### D2: Terminal Lifecycle

```
main() → install panic hook → TTY check → setup_terminal() → [wizard?] → run() → restore_terminal()
                 ↓ (on panic)
         restore_terminal() → original_hook()
```

`setup_terminal()` enables raw mode, enters alternate screen, enables mouse capture. `restore_terminal()` reverses all three. The panic hook calls `restore_terminal()` before the original hook, ensuring the developer's shell is never left in raw mode.

### D3: Configuration

TOML file at `dirs::config_dir()/swink-agent/tui.toml`. Uses `serde(default)` so any subset of fields can be specified. Unknown keys are silently ignored. Invalid TOML falls back to full defaults with a tracing warning.

Fields: `show_thinking` (bool), `auto_scroll` (bool), `tick_rate_ms` (u64, default 33), `default_model` (string), `theme` (string), `system_prompt` (optional string), `editor_command` (optional string), `color_mode` (string: custom/mono-white/mono-black).

### D4: Credential Resolution

Priority chain per provider: environment variable → OS keychain. When the keychain is unavailable, only environment variables are supported (no local file fallback).

Provider priority order (highest to lowest): Custom Proxy (`LLM_BASE_URL` + `LLM_API_KEY`) → OpenAI (`OPENAI_API_KEY`) → Anthropic (`ANTHROPIC_API_KEY`) → Local LLM (feature-gated) → Ollama (no key needed, fallback).

### D5: Color Theme System

Three modes cycled via F3: Custom (distinct colors per role), MonoWhite (all colors → White), MonoBlack (all colors → Black). Monochrome modes preserve modifiers (bold, dim, underline) for semantic differentiation. All color access goes through `resolve()` which applies the current mode.

Process-global `AtomicU8` for mode state — acceptable because color mode is display-only and process-scoped.

### D6: Focus Management

`Focus` enum with variants (initially Input and Conversation). Tab cycles forward, Shift+Tab cycles backward. Focused component gets `border_focused_color()` (White), unfocused gets `border_color()` (DarkGray). Component-specific shortcuts only fire when that component has focus.

### D7: Minimum Terminal Size

`render()` checks terminal dimensions before drawing. If below 120×30, renders a centered warning overlay instead of the normal UI. Warning includes the current size and the minimum required. Normal UI resumes immediately when the terminal is resized above the threshold.

### D8: Non-Interactive Terminal Detection

At the start of `main()`, check if stdout is a TTY. If not (piped, redirected, no terminal), print a clear error message to stderr and exit with code 1. Use crossterm's capabilities or `std::io::IsTerminal` (stable since Rust 1.70).

### D9: System Prompt Resolution

Priority chain: explicit parameter → `LLM_SYSTEM_PROMPT` env var → config file `system_prompt` field → default constant ("You are a helpful assistant.").

### D10: Logging Strategy

TUI owns stdout — all logging goes to files via `tracing-appender`. Rolling daily logs at `dirs::config_dir()/swink-agent/logs/swink-agent.log`. Log level controlled via `RUST_LOG` env var, defaulting to `swink_agent=info`.

## Complexity Tracking

No constitution violations. All work fits within the existing workspace structure as the 7th crate member (constitution allows 7).
