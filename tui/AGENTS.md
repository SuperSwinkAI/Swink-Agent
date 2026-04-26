# AGENTS.md — Terminal UI

## Scope

`tui/` — Interactive terminal binary (`swink`). ratatui + crossterm. See `tui/src/ui/AGENTS.md` for rendering.

## Key Facts

- Event loop: `tokio::select!` — terminal, agent, approval, tick. Dirty flag redraws.
- Credentials: env var first, then keychain. F4 cycles model. Plan mode delegates to core.
- Keybindings: F1 help, F2 inspect tool, F3 color mode, F4 model. Click+drag selects (release copies). Ctrl+C copies/aborts. Esc clears/aborts.

## Key Invariants

- **Mid-stream input uses `agent.steer()`** — queued at turn boundary, shown in "Queued" overlay. Flushed at `MessageStart`. Do not abort the stream.
- **Approval fails closed** — `tui_approval_callback()` returns `Rejected` on any plumbing failure.
- **Mouse capture blocks native selection** — in-app selection workaround with `Modifier::REVERSED` post-render pass.
- **Session restore** — use `load_full()` for atomic transcript+state. Validate before mutating. Preserve `is_error` on tool results. `set_agent()` before `resume_into()`.
- **`approval_mode` owned by `Agent`** — no `App.approval_mode` field; read through `agent.approval_mode()`.
- **`#key` handling** — single parser for detection and execution. Unparseable secret input fails closed. Bare `#key` returns `None`.
- Panic hook restores terminal. External editor temp files randomized via `tempfile`.
