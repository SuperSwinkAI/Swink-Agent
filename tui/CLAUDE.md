# CLAUDE.md — Terminal UI

## Scope

`tui/` — Interactive terminal binary (`swink`). ratatui + crossterm. See `tui/src/ui/CLAUDE.md` for rendering components.

## Key Facts

- Provider priority: Proxy (`LLM_BASE_URL`) > OpenAI (`OPENAI_API_KEY`) > Anthropic (`ANTHROPIC_API_KEY`) > Ollama (default fallback).
- Event loop (`app.rs`): `tokio::select!` — terminal events, agent events, approval events, then tick.
- Dirty flag optimization: frame redraws only when `dirty = true`.
- Plan mode delegates to `agent.enter_plan_mode()`/`exit_plan_mode()` in core; TUI just manages UI state.

## Lessons Learned

- **`/model` changes model ID, not provider** — to switch providers, update `.env` and restart.
- **Panic hook restores terminal** — without it, a panic leaves terminal in raw mode.
- **Credentials** — env var checked first, then keychain. Env always wins.
