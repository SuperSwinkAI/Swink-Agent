# Agent Harness

A pure-Rust library for building LLM-powered agentic loops. Provider-agnostic core with pluggable streaming, concurrent tool execution, and lifecycle events.

## Workspace

| Crate | Type | Purpose |
|---|---|---|
| `agent-harness` | lib | Agent loop, tool system, streaming traits, retry, error types |
| `agent-harness-adapters` | lib | `StreamFn` adapters — Ollama, Anthropic, OpenAI |
| `agent-harness-tui` | bin | Interactive terminal UI with markdown, syntax highlighting, tool panel |

## Key Ideas

- **`StreamFn`** is the only provider boundary — implement it to add a new LLM backend.
- **`AgentTool`** trait + JSON Schema validation for tool definitions.
- Tools execute concurrently within a turn; steering callbacks can interrupt mid-batch.
- Errors stay in the message log — the loop keeps running. Typed `HarnessError` variants for callers.
- Events are push-only (`AgentEvent` stream). No inward mutation through events.
- No `unsafe` code. No global mutable state.

## Quick Reference

```bash
cargo run -p agent-harness-tui     # launch the TUI
cargo test --workspace             # run all tests
```

See [docs/getting_started.md](docs/getting_started.md) for setup and configuration.
See [docs/architecture/HLD.md](docs/architecture/HLD.md) for system design.
See [docs/planning/PRD.md](docs/planning/PRD.md) for product requirements.
