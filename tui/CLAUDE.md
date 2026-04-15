# CLAUDE.md — Terminal UI

## Scope

`tui/` — Interactive terminal binary (`swink`). ratatui + crossterm. See `tui/src/ui/CLAUDE.md` for rendering components.

## Key Facts

- `credentials::providers()` returns entries in a fixed iteration order (Ollama, OpenAI, Anthropic, Custom Proxy, Local SmolLM3-3B). This order affects the F4 model-cycle UI. The active model is still determined by agent configuration.
- Event loop (`app.rs`): `tokio::select!` — terminal events, agent events, approval events, then tick.
- Dirty flag optimization: frame redraws only when `dirty = true`.
- Plan mode delegates to `agent.enter_plan_mode()`/`exit_plan_mode()` in core; TUI just manages UI state.

## Keybindings

- **F1** — Toggle help panel.
- **F2** — Inspect selected tool block.
- **F3** — Cycle color mode.
- **F4** — Cycle model (applied on next send).

## Lessons Learned

- **Approval mode is owned by `Agent`** — `App::approval_mode()` reads through to `agent.approval_mode()` and returns `Smart` before `set_agent` is called. Do not add an `App.approval_mode` field; doing so reintroduces the dual-write drift bug (#565). Configure the startup mode via `AgentOptions::with_approval_mode` before building the agent.
- **Mid-stream user input uses `agent.steer()`** — when `status == Running`, `send_to_agent` calls `agent.steer()` instead of `prompt_stream` (which would error). The text is held in `App::pending_steered` and shown in a "Queued" overlay above the input. Messages are queued immediately and delivered at the next turn boundary — the agent completes its current LLM response first, then processes the steering message. Do NOT abort the LLM stream; this is the correct pattern for text agents (researched 2026-04-15). Flush timing: `pending_steered` is drained into `self.messages` at `MessageStart` (not `AgentEnd`) so the user message appears immediately before the assistant turn that responds to it — correct chronological order. `AgentEnd` has a safety flush only for cancelled turns. `steered_fade_ticks` triggers a brief fade-out of the overlay after delivery. Do not push `DisplayMessage::User` in `submit_input` while running.
- **Smart approval mode auto-approves trusted tools only** — untrusted tools still prompt even if they are read-only.
- **Panic hook restores terminal** — without it, a panic leaves terminal in raw mode.
- **Credentials** — env var checked first, then keychain. Env always wins.
