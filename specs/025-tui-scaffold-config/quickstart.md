# Quickstart: TUI: Scaffold, Event Loop & Config

## Prerequisites

- Rust 1.88+ with edition 2024
- Workspace built: `cargo build --workspace`
- At least one provider credential configured (or Ollama running locally)

## Build & Run

```bash
# Build the TUI binary
cargo build -p swink-agent-tui

# Run (auto-loads .env)
cargo run -p swink-agent-tui

# Run without local LLM feature (faster compile)
cargo run -p swink-agent-tui --no-default-features
```

## First Run

If no API keys are configured, the setup wizard launches automatically:

1. Select a provider (Ollama, OpenAI, Anthropic, Custom Proxy)
2. Enter your API key (stored in OS keychain)
3. Press Enter to start the TUI

To skip the wizard, set an environment variable:
```bash
export OPENAI_API_KEY="sk-..."
cargo run -p swink-agent-tui
```

## Configuration

Config file: `~/.config/swink-agent/tui.toml` (macOS/Linux) or `%APPDATA%/swink-agent/tui.toml` (Windows).

```toml
# Example configuration
show_thinking = true
auto_scroll = true
tick_rate_ms = 33         # 30 FPS (default)
default_model = "gpt-4o"
theme = "default"
color_mode = "custom"     # "custom", "mono-white", "mono-black"
# system_prompt = "You are a coding assistant."
# editor_command = "nano"
```

Missing fields use defaults. Unknown keys are ignored. Invalid TOML falls back to full defaults.

## Key Bindings

| Key | Action |
|-----|--------|
| Ctrl+Q | Quit |
| Ctrl+C | Cancel running agent (or quit if idle) |
| Tab | Cycle focus (Input → Conversation) |
| Shift+Tab | Toggle operating mode (Execute ↔ Plan) |
| F1 | Toggle help panel |
| F3 | Cycle color mode |
| F4 | Cycle model |
| Enter | Send message |
| Shift+Enter | Insert newline |

## Provider Priority

When multiple credentials are available, the TUI selects the highest-priority provider:

1. **Custom Proxy** — `LLM_BASE_URL` + `LLM_API_KEY`
2. **OpenAI** — `OPENAI_API_KEY` (env or keychain)
3. **Anthropic** — `ANTHROPIC_API_KEY` (env or keychain)
4. **Local LLM** — Built-in (feature-gated: `local`)
5. **Ollama** — No key needed, `OLLAMA_HOST` (default: `localhost:11434`)

Environment variables always override keychain values.

## Testing

```bash
# Unit tests
cargo test -p swink-agent-tui

# With no default features
cargo test -p swink-agent-tui --no-default-features
```

## Logging

Logs are written to `~/.config/swink-agent/logs/swink-agent.log` (rolling daily). Control log level via:

```bash
RUST_LOG=swink_agent=debug cargo run -p swink-agent-tui
```
