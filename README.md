# Swink Agent

A pure-Rust library for building LLM-powered agentic loops. Provider-agnostic core with pluggable streaming, concurrent tool execution, and lifecycle events.

## Workspace

| Crate | Type | Purpose |
|---|---|---|
| `swink-agent` | lib | Agent loop, tool system, streaming traits, retry, error types |
| `swink-agent-adapters` | lib | `StreamFn` adapters — Anthropic, OpenAI, Google Gemini, Ollama, Azure, xAI, Mistral, Bedrock |
| `swink-agent-memory` | lib | Session persistence, summarization compaction |
| `swink-agent-local-llm` | lib | On-device inference via SmolLM3-3B (text/tools) and EmbeddingGemma-300M (embeddings) |
| `swink-agent-eval` | lib | Evaluation harness — efficiency scoring, budget guards, gate checks, audit trails |
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

## Example: Build a Custom Agent

Wire up an LLM provider, register tools, and launch the interactive TUI — all in one file:

```rust
use std::sync::Arc;

use swink_agent::{Agent, AgentOptions, AgentTool, BashTool, ModelConnections, ReadFileTool, WriteFileTool};
use swink_agent_adapters::{build_remote_connection, remote_preset_keys};
use swink_agent_local_llm::default_local_connection;
use swink_agent_tui::{
    TuiConfig, launch, restore_terminal, setup_terminal, tui_approval_callback,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    let connections = ModelConnections::new(
        build_remote_connection(remote_preset_keys::anthropic::SONNET_46)?,
        vec![
            build_remote_connection(remote_preset_keys::openai::GPT_5_2)?,
            default_local_connection()?,
        ],
    );
    let (model, stream_fn, extra_models) = connections.into_parts();
    let tools: Vec<Arc<dyn AgentTool>> = vec![
        Arc::new(BashTool::new()),
        Arc::new(ReadFileTool::new()),
        Arc::new(WriteFileTool::new()),
    ];

    let mut terminal = setup_terminal()?;

    let result = launch(TuiConfig::default(), &mut terminal, |approval_tx| {
        let options = AgentOptions::new(
            "You are a helpful coding assistant.",
            model,
            stream_fn,
            swink_agent::default_convert,
        )
        .with_available_models(extra_models)
        .with_tools(tools)
        .with_approve_tool(tui_approval_callback(approval_tx));

        Agent::new(options)
    })
    .await;

    restore_terminal()?;
    result
}
```

```bash
cargo run --example custom_agent
```

## More Examples

Runnable examples live in `examples/`:

| Example | What it demonstrates |
|---|---|
| [`custom_agent`](examples/custom_agent.rs) | Full agent with Anthropic adapter, tools, and interactive TUI |
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
