# Swink Agent

[![CI](https://github.com/SuperSwinkAI/Swink-Agent/actions/workflows/ci.yml/badge.svg)](https://github.com/SuperSwinkAI/Swink-Agent/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/swink-agent.svg)](https://crates.io/crates/swink-agent)
[![Docs.rs](https://docs.rs/swink-agent/badge.svg)](https://docs.rs/swink-agent)
[![MSRV](https://img.shields.io/badge/rustc-1.95+-blue.svg)](https://blog.rust-lang.org/2026/04/16/Rust-1.95.0/)
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
| `swink-agent-eval` | lib + bin | Evaluation harness — efficiency scoring, budget guards, gate checks, audit trails, plus 24 judge-backed/deterministic evaluators, multi-turn simulation, trace ingestion (OTLP / Langfuse / OpenSearch / CloudWatch), Console/JSON/Markdown/HTML/LangSmith reporters, and the `swink-eval` CLI (`cli` feature) |
| `swink-agent-eval-judges` | lib | Per-provider `JudgeClient` implementations (Anthropic, OpenAI, Bedrock, Gemini, Mistral, Azure, xAI, Ollama, Proxy) with `Blocking<Provider>JudgeClient` sync wrappers behind feature flags |
| `swink-agent-evolve` | lib | Eval-driven self-improvement loop for prompts and tool schemas (Spec 044) |
| `swink-agent-artifacts` | lib | Versioned artifact storage (filesystem + in-memory backends) |
| `swink-agent-auth` | lib | OAuth2 credential management and refresh |
| `swink-agent-mcp` | lib | Model Context Protocol integration (stdio/SSE) |
| `swink-agent-patterns` | lib | Multi-agent orchestration patterns (pipeline, parallel, loop) |
| `swink-agent-plugin-web` | lib | Web browsing and search plugin |
| `swink-agent-rpc` | lib + bin | JSON-RPC 2.0 agent service and `swink-agentd` daemon over Unix sockets (Spec 045) |
| `swink-agent-macros` | proc-macro | `#[derive(ToolSchema)]` and `#[tool]` proc macros |
| `swink-agent-tui` | lib + bin | Interactive terminal UI with markdown, syntax highlighting, tool panel |
| `xtask` | bin | Developer workflow tasks such as catalog and release validation |

## Feature Matrix

| Surface | Default | Opt-in features | Notes |
|---|---|---|---|
| `swink-agent` | `builtin-tools`, `transfer` | `artifact-store`, `artifact-tools`, `hot-reload`, `plugins`, `testkit`, `tiktoken`, `otel` | Core loop and tool/runtime surface stay provider-agnostic. |
| `swink-agent-adapters` | Core remote adapters | Per-provider adapter features | Remote HTTP/SSE adapters are isolated from the core crate. |
| `swink-agent-policies` | `all` | Individual policy features such as `budget`, `sandbox`, `audit`, `pii` | Reusable loop/app policies stay independently gateable. |
| `swink-agent-local-llm` | SmolLM3 local runtime | Backend/model features such as `cuda`, `metal`, `vulkan`, `gemma4` | Requires LLVM/libclang because `llama-cpp-sys-2` runs `bindgen`. |
| `swink-agent-eval` | Spec 023 deterministic evals | `judge-core`, `all-evaluators`, `simulation`, `generation`, `trace-ingest`, `trace-otlp`, `trace-langfuse`, `trace-opensearch`, `trace-cloudwatch`, `telemetry`, `html-report`, `langsmith`, `cli`, `multimodal`, `yaml` | Advanced evals remain mostly opt-in so no-default builds stay slim. |
| `swink-agent-eval-judges` | Minimal shared client surface | Per-provider judge features plus `all-judges` | Provider credentials and transport details live in the separate judge-client crate. |
| `swink-agent-tui` | Terminal UI with remote providers | `local`, `full` | `local` brings in on-device inference; `full` enables the broader optional surface. |

## Key Ideas

- **`StreamFn`** is the only provider boundary — implement it to add a new LLM backend.
- **`AgentTool`** trait + JSON Schema validation for tool definitions.
- Tools execute concurrently within a turn; steering callbacks can interrupt mid-batch.
- Errors stay in the message log — the loop keeps running. Typed `AgentError` variants for callers.
- Events are push-only (`AgentEvent` stream). No inward mutation through events.
- No `unsafe` code. No global mutable state.
- Optional `tiktoken` support ships a built-in `TiktokenCounter` for more accurate context budgets.

## Quick Reference

```bash
cargo run -p swink-agent-tui     # launch the TUI (remote/Ollama defaults)
cargo run -p swink-agent-tui --features local  # include bundled local-LLM support
cargo run -p swink-agent-eval --features cli --bin swink-eval -- --help
cargo test --workspace             # run all tests
just validate                      # formatting, clippy, tests, package preflight
```

Workspace-wide `cargo build/test/clippy` commands also compile `swink-agent-local-llm`. That crate currently depends on `llama-cpp-sys-2`, so contributor machines need LLVM/libclang available for `bindgen`; if auto-discovery fails, set `LIBCLANG_PATH` to the LLVM `bin` directory before running workspace checks.

For the advanced evaluation surface, see [`eval/README.md`](eval/README.md) for evaluator/reporter usage and [`eval-judges/README.md`](eval-judges/README.md) for provider feature flags and credential setup.

## Hot-Reloaded Script Tools

When the `hot-reload` feature is enabled, `ScriptTool` definitions use a
top-level `[parameters_schema]` TOML section, and `command` interpolates
runtime arguments with `{param}` placeholders:

```toml
name = "greet"
description = "Greet a person by name"
command = "echo 'Hello, {name}!'"

[parameters_schema]
type = "object"
required = ["name"]

[parameters_schema.properties.name]
type = "string"
description = "The name to greet"
```

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
