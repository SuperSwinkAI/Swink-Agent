# swink-agent-adapters

[![Crates.io](https://img.shields.io/crates/v/swink-agent-adapters.svg)](https://crates.io/crates/swink-agent-adapters)
[![Docs.rs](https://docs.rs/swink-agent-adapters/badge.svg)](https://docs.rs/swink-agent-adapters)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](https://github.com/SuperSwinkAI/Swink-Agent/blob/main/LICENSE-MIT)

LLM provider adapters for [`swink-agent`](https://crates.io/crates/swink-agent) — one `StreamFn` per backend with identical streaming, tool-call, and error semantics.

## Features

- **Anthropic** (`anthropic`) — Messages API with streaming, tool use, and extended thinking
- **OpenAI** (`openai`) — Responses API and Chat Completions with streaming tool calls
- **Google Gemini** (`gemini`) — streaming generation with function calling
- **Ollama** (`ollama`) — local OpenAI-compatible inference servers
- **Azure OpenAI** (`azure`) — AAD token auth via [`swink-agent-auth`](https://crates.io/crates/swink-agent-auth)
- **AWS Bedrock** (`bedrock`) — SigV4-signed event streams (Claude, Titan, Llama)
- **Mistral** (`mistral`) and **xAI** (`xai`) — first-party and OpenAI-compatible endpoints
- **Proxy** (`proxy`) — route through a gateway or replay recorded fixtures in tests
- Catalog lookup: `build_remote_connection_for_model("claude-sonnet-4-6")` returns a ready `ModelConnection`
- Cargo features are fully opt-in — enable only the providers you ship

## Quick Start

```toml
[dependencies]
swink-agent = "0.9.0"
swink-agent-adapters = { version = "0.9.0", features = ["anthropic", "openai"] }
tokio = { version = "1", features = ["full"] }
```

```rust
use swink_agent::prelude::*;
use swink_agent_adapters::build_remote_connection_for_model;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let connections = ModelConnections::builder()
        .primary(build_remote_connection_for_model("claude-sonnet-4-6")?)
        .fallback(build_remote_connection_for_model("gpt-4.1")?)
        .build();

    let options = AgentOptions::from_connections(
        "You are a helpful assistant.",
        connections,
    )
    .with_default_tools();

    let mut agent = Agent::new(options);
    let result = agent.prompt_text("Summarize the README in one sentence.").await?;
    println!("{}", result.assistant_text());
    Ok(())
}
```

## Architecture

Each provider is a thin `StreamFn` implementation that maps the vendor's native streaming wire format onto `swink-agent`'s unified `AssistantMessageEvent` stream. Shared plumbing (model presets, retry policies, credential resolution) lives in the crate root; per-provider modules contain only wire-level conversion. Providers that need OAuth or signed requests pull `swink-agent-auth` or the `aws-sigv4` stack in through feature-gated dependencies, so a build with just `anthropic` stays small.

No `unsafe` code (`#![forbid(unsafe_code)]`). No global state — every adapter owns its `reqwest::Client` and is constructed explicitly through a preset or builder.

---

Part of the [swink-agent](https://github.com/SuperSwinkAI/Swink-Agent) workspace — see the [main README](https://github.com/SuperSwinkAI/Swink-Agent#readme) for workspace overview and setup.
