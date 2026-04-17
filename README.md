# Swink Agent

[![CI](https://github.com/SuperSwinkAI/Swink-Agent/actions/workflows/ci.yml/badge.svg)](https://github.com/SuperSwinkAI/Swink-Agent/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/swink-agent.svg)](https://crates.io/crates/swink-agent)
[![Docs.rs](https://docs.rs/swink-agent/badge.svg)](https://docs.rs/swink-agent)
[![MSRV](https://img.shields.io/badge/rustc-1.88+-blue.svg)](https://blog.rust-lang.org/2025/06/05/Rust-1.88.0/)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)

A pure-Rust library for building LLM-powered agentic loops. Provider-agnostic core with pluggable streaming, concurrent tool execution, and lifecycle events.

## Workspace

| Crate | Type | Purpose |
|---|---|---|
| `swink-agent` | lib | Agent loop, tool system, streaming traits, retry, error types |
| `swink-agent-adapters` | lib | `StreamFn` adapters — Anthropic, OpenAI, Google Gemini, Ollama, Azure, xAI, Mistral, Bedrock |
| `swink-agent-policies` | lib | 10 feature-gated policy implementations (budget, sandbox, PII, audit, etc.) |
| `swink-agent-memory` | lib | Session persistence, summarization compaction |
| `swink-agent-local-llm` | lib | On-device inference — SmolLM3-3B (default), Gemma 4 (opt-in `gemma4` feature), EmbeddingGemma-300M (embeddings) |
| `swink-agent-eval` | lib | Evaluation harness — efficiency scoring, budget guards, gate checks, audit trails |
| `swink-agent-artifacts` | lib | Versioned artifact storage (filesystem + in-memory backends) |
| `swink-agent-auth` | lib | OAuth2 credential management and refresh |
| `swink-agent-mcp` | lib | Model Context Protocol integration (stdio/SSE) |
| `swink-agent-patterns` | lib | Multi-agent orchestration patterns (pipeline, parallel, loop) |
| `swink-agent-plugin-web` | lib | Web browsing and search plugin |
| `swink-agent-macros` | proc-macro | `#[derive(ToolSchema)]` and `#[tool]` proc macros |
| `swink-agent-tui` | lib + bin | Interactive terminal UI with markdown, syntax highlighting, tool panel |

## Key Ideas

- **`StreamFn`** is the only provider boundary — implement it to add a new LLM backend.
- **`AgentTool`** trait + JSON Schema validation for tool definitions.
- Tools execute concurrently within a turn; steering callbacks can interrupt mid-batch.
- Errors stay in the message log — the loop keeps running. Typed `AgentError` variants for callers.
- Events are push-only (`AgentEvent` stream). No inward mutation through events.
- No `unsafe` code. No global mutable state.

## Quick Reference

```bash
cargo run -p swink-agent-tui     # launch the TUI (remote/Ollama defaults)
cargo run -p swink-agent-tui --features local  # include bundled local-LLM support
cargo test --workspace             # run all tests
```

Workspace-wide `cargo build/test/clippy` commands also compile `swink-agent-local-llm`. That crate currently depends on `llama-cpp-sys-2`, so contributor machines need LLVM/libclang available for `bindgen`; if auto-discovery fails, set `LIBCLANG_PATH` to the LLVM `bin` directory before running workspace checks.

## Example: Build a Custom Agent

Wire up an LLM provider, register tools, and launch the interactive TUI — all in one file:

```rust
use swink_agent::{AgentOptions, BashTool, ModelConnections, ReadFileTool, WriteFileTool};
use swink_agent_adapters::build_remote_connection_for_model;
use swink_agent_local_llm::default_local_connection;
use swink_agent_tui::{TuiConfig, launch, restore_terminal, setup_terminal};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    let connections = ModelConnections::new(
        build_remote_connection_for_model("claude-sonnet-4-6")?,
        vec![
            build_remote_connection_for_model("gpt-4.1")?,
            default_local_connection()?,
        ],
    );
    let tools = vec![
        BashTool::new().into_tool(),
        ReadFileTool::new().into_tool(),
        WriteFileTool::new().into_tool(),
    ];

    let options =
        AgentOptions::from_connections("You are a helpful assistant.", connections)
            .with_tools(tools);

    let mut terminal = setup_terminal()?;
    let result = launch(TuiConfig::default(), &mut terminal, options).await;

    restore_terminal()?;
    result
}
```

## More Examples

Runnable examples are in [SuperSwinkAI/Swink-Agent-Examples](https://github.com/SuperSwinkAI/Swink-Agent-Examples):

| Example | What it demonstrates |
|---|---|
| `simple_prompt` | Create an Agent with a mock stream function, send a prompt, print the result |
| `with_tools` | Register BashTool / ReadFileTool / WriteFileTool and wire up the approval callback |
| `custom_adapter` | Implement the `StreamFn` trait for a custom provider |
| `custom_agent` | Full agent with Anthropic adapter, tools, and interactive TUI |

See [docs/getting_started.md](docs/getting_started.md) for setup and configuration.
See [docs/architecture/HLD.md](docs/architecture/HLD.md) for system design.
See [docs/planning/PRD.md](docs/planning/PRD.md) for product requirements.
