# CLAUDE.md — Terminal UI

## Scope

`tui/` — Interactive terminal binary (`swink`). ratatui + crossterm. See `tui/src/ui/CLAUDE.md` for rendering components.

## Key Facts

- Provider configuration depends on the specific agent setup; `credentials::providers()` enumerates available credentials but there is no fixed priority — the active model is determined by agent configuration.
- Event loop (`app.rs`): `tokio::select!` — terminal events, agent events, approval events, then tick.
- Dirty flag optimization: frame redraws only when `dirty = true`.
- Plan mode delegates to `agent.enter_plan_mode()`/`exit_plan_mode()` in core; TUI just manages UI state.

## Keybindings

- **F1** — Toggle help panel.
- **F2** — Inspect selected tool block.
- **F3** — Cycle color mode.
- **F4** — Cycle model (applied on next send).

## Lessons Learned

- **`/model` changes model ID, not provider** — to switch providers, update `.env` and restart.
- **Panic hook restores terminal** — without it, a panic leaves terminal in raw mode.
- **Credentials** — env var checked first, then keychain. Env always wins.
