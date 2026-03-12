# Swink Agent

A pure-Rust library for building LLM-powered agentic loops. Provider-agnostic core with pluggable streaming, concurrent tool execution, and lifecycle events.

## Workspace

| Crate | Type | Purpose |
|---|---|---|
| `swink-agent` | lib | Agent loop, tool system, streaming traits, retry, error types |
| `swink-agent-adapters` | lib | `StreamFn` adapters — Ollama, Anthropic, OpenAI |
| `swink-agent-tui` | bin | Interactive terminal UI with markdown, syntax highlighting, tool panel |

## Key Ideas

- **`StreamFn`** is the only provider boundary — implement it to add a new LLM backend.
- **`AgentTool`** trait + JSON Schema validation for tool definitions.
- Tools execute concurrently within a turn; steering callbacks can interrupt mid-batch.
- Errors stay in the message log — the loop keeps running. Typed `AgentError` variants for callers.
- Events are push-only (`AgentEvent` stream). No inward mutation through events.
- No `unsafe` code. No global mutable state.

## Quick Reference

```bash
cargo run -p swink-agent-tui     # launch the TUI
cargo test --workspace             # run all tests
```

## Examples

Runnable examples live in `examples/`:

| Example | What it demonstrates |
|---|---|
| [`simple_prompt`](examples/simple_prompt.rs) | Create an Agent with a mock stream function, send a prompt, print the result |
| [`with_tools`](examples/with_tools.rs) | Register BashTool / ReadFileTool / WriteFileTool and wire up the approval callback |
| [`custom_adapter`](examples/custom_adapter.rs) | Implement the `StreamFn` trait for a custom provider |

```bash
cargo run --example simple_prompt
cargo run --example with_tools
cargo run --example custom_adapter
```

See [docs/getting_started.md](docs/getting_started.md) for setup and configuration.
See [docs/architecture/HLD.md](docs/architecture/HLD.md) for system design.
See [docs/planning/PRD.md](docs/planning/PRD.md) for product requirements.
