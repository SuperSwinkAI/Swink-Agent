# Quickstart: Adapter — Mistral

**Feature**: 018-adapter-mistral
**Date**: 2026-03-30

## Prerequisites

- Rust 1.88+ (edition 2024)
- Mistral API key (from https://console.mistral.ai/)
- `swink-agent-adapters` crate with `mistral` feature enabled

## Setup

### Cargo.toml

```toml
[dependencies]
swink-agent = { path = "../path/to/swink-agent" }
swink-agent-adapters = { path = "../path/to/adapters", features = ["mistral"] }
tokio = { version = "1", features = ["full"] }
```

### Environment

```bash
export MISTRAL_API_KEY="your-key-here"
```

## Direct Usage

```rust
use swink_agent::Agent;
use swink_agent_adapters::MistralStreamFn;

#[tokio::main]
async fn main() {
    let stream_fn = MistralStreamFn::new(
        "https://api.mistral.ai",
        std::env::var("MISTRAL_API_KEY").unwrap(),
    );

    let agent = Agent::new(stream_fn)
        .with_system_prompt("You are a helpful assistant.");

    let response = agent.prompt("Hello!").await;
    println!("{}", response.text());
}
```

## Preset Usage

```rust
use swink_agent::Agent;
use swink_agent_adapters::remote_preset_keys::mistral;
use swink_agent_adapters::build_remote_connection;

#[tokio::main]
async fn main() {
    // Reads MISTRAL_API_KEY from environment automatically
    let connection = build_remote_connection(mistral::MISTRAL_SMALL).unwrap();

    let agent = Agent::new(connection.stream_fn)
        .with_model(connection.model_spec)
        .with_system_prompt("You are a helpful assistant.");

    let response = agent.prompt("Hello!").await;
    println!("{}", response.text());
}
```

## With Tools

```rust
use swink_agent::{Agent, AgentTool, AgentToolResult};
use swink_agent_adapters::MistralStreamFn;

// Define your tool (implements AgentTool trait)
let stream_fn = MistralStreamFn::new(
    "https://api.mistral.ai",
    std::env::var("MISTRAL_API_KEY").unwrap(),
);

let agent = Agent::new(stream_fn)
    .with_tool(my_tool);
```

## Running Tests

```bash
# Unit tests (mock server, no API key needed)
cargo test -p swink-agent-adapters --features mistral

# Live integration test (requires MISTRAL_API_KEY)
cargo test -p swink-agent-adapters --test mistral_live -- --ignored
```

## Available Presets

| Preset | Model | Best For |
|---|---|---|
| `mistral::MISTRAL_LARGE` | mistral-large-latest | Complex reasoning, vision |
| `mistral::MISTRAL_SMALL` | mistral-small-latest | Fast general-purpose |
| `mistral::CODESTRAL` | codestral-latest | Code generation |
| `mistral::PIXTRAL_LARGE` | pixtral-large-2411 | Vision tasks |
