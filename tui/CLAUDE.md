# CLAUDE.md — Terminal UI

## Scope

`tui/` — Interactive terminal binary (`agent-tui`). Uses ratatui + crossterm for rendering, connects to LLM providers via adapters crate.

## References

- **PRD:** §16 (TUI), §16.1–§16.5
- **Architecture:** `docs/architecture/tui/README.md`

## Key Facts

- Binary name is `agent-tui` (defined in `tui/Cargo.toml`).
- `dotenvy::dotenv().ok()` runs first in `main()` — `.env` loads automatically before any env var reads.
- Provider priority: Proxy (`LLM_BASE_URL`) > OpenAI (`OPENAI_API_KEY`) > Anthropic (`ANTHROPIC_API_KEY`) > Ollama (default). Each branch returns early.
- If no provider is configured, falls back to Ollama. Error shows on first message if Ollama isn't running — app doesn't crash.
- Setup wizard runs when `!credentials::any_key_configured()`. User can quit from wizard.
- All paths use `dirs::config_dir()` — cross-platform (macOS, Linux, Windows).
- Session and log directories are created automatically via `create_dir_all`.

## Event Loop

`app.rs` uses `tokio::select!` with `biased;`:
1. Terminal events (keyboard/mouse/resize) — checked first
2. Agent events (from mpsc channel)
3. Tick interval (animations)

**Dirty flag optimization** — frame only redraws when `dirty = true`. Tick sets dirty only during active agent runs or when tool panel is visible. Prevents unnecessary CPU usage.

## Modules

| File | Purpose |
|---|---|
| `main.rs` | Entry point, provider selection, terminal setup/teardown |
| `app.rs` | Event loop, state management, agent integration |
| `commands.rs` | Hash (#) and slash (/) command parsing |
| `config.rs` | TuiConfig from TOML (~/.config/agent-harness/tui.toml) |
| `credentials.rs` | OS keychain via keyring crate |
| `session.rs` | JSONL session persistence |
| `wizard.rs` | First-run setup wizard |
| `format.rs` | Token/time formatting helpers |
| `theme.rs` | Color constants |
| `ui/` | Rendering components (see `tui/src/ui/CLAUDE.md`) |

## Lessons Learned

- **`/model` changes model ID within the current provider** — it does NOT switch providers. To test a different provider, update `.env` and restart.
- **Wizard catches keyring failures gracefully** — `store_credential` errors are caught, user can retry. Handles Linux without secret-service.
- **Panic hook restores terminal** — `main.rs` installs a panic hook that calls `restore_terminal()` before unwinding. Without this, a panic leaves the terminal in raw mode.
- **`biased;` in select! matters** — ensures terminal events (user input) are always processed before agent events or ticks. Without it, heavy streaming could starve input handling.
- **Credentials check order** — `credential()` checks env var first, then keychain. Explicit env override always wins.
