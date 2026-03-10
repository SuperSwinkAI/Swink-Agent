# Agent Harness

A pure-Rust agent harness for running LLM-powered agentic loops. Provider-agnostic core with pluggable streaming, tool execution, and lifecycle events.

---

## Workspace

```
agent-harness/          Core library — types, traits, agent loop, proxy streaming
adapters/               LLM provider adapters (Ollama, future: Anthropic, OpenAI, …)
tui/                    Terminal UI binary — interactive chat interface
```

| Crate | Type | Description |
|---|---|---|
| `agent-harness` | lib | Agent loop, tool system, streaming traits, proxy `StreamFn`, retry strategy |
| `agent-harness-adapters` | lib | Provider-specific `StreamFn` implementations (currently: `OllamaStreamFn`) |
| `agent-harness-tui` | bin | Full-featured TUI with markdown rendering, syntax highlighting, tool panel |

---

## Quick Start

Requires [Ollama](https://ollama.ai) running locally with a model pulled (default: `llama3.2`).

```bash
# Start Ollama (if not already running)
ollama serve

# Pull a model
ollama pull llama3.2

# Run the TUI
cargo run -p agent-harness-tui
```

### Environment Variables

The TUI reads configuration from environment variables. If `LLM_BASE_URL` is set, the TUI uses the proxy `StreamFn` instead of Ollama.

| Variable | Default | Description |
|---|---|---|
| `OLLAMA_HOST` | `http://localhost:11434` | Ollama server URL |
| `OLLAMA_MODEL` | `llama3.2` | Ollama model name |
| `LLM_BASE_URL` | — | Proxy server URL (overrides Ollama mode) |
| `LLM_API_KEY` | — | Bearer token for proxy authentication |
| `LLM_MODEL` | `claude-sonnet-4-20250514` | Model ID when using proxy mode |
| `LLM_SYSTEM_PROMPT` | — | Custom system prompt |

---

## Tests

The core library has 144 tests covering all six implementation phases.

```bash
# Run all workspace tests
cargo test --workspace

# Run core library tests only
cargo test -p agent-harness
```

---

## Architecture

See [`docs/planning/PRD.md`](docs/planning/PRD.md) for the product requirements and [`docs/planning/IMPLEMENTATION_PHASES.md`](docs/planning/IMPLEMENTATION_PHASES.md) for the phased implementation plan.

No `unsafe` code. No global mutable state.
