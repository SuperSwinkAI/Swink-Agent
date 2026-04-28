# swink-agent-tui

[![Crates.io](https://img.shields.io/crates/v/swink-agent-tui.svg)](https://crates.io/crates/swink-agent-tui)
[![Docs.rs](https://docs.rs/swink-agent-tui/badge.svg)](https://docs.rs/swink-agent-tui)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](https://github.com/SuperSwinkAI/Swink-Agent/blob/main/LICENSE-MIT)

Interactive terminal UI for [`swink-agent`](https://crates.io/crates/swink-agent) — chat, tool approvals, session history, and credential wizard in one `ratatui` app. Ships as both a library and a `swink` binary.

## Features

- **`swink` binary** (`cli` feature, default) — zero-config launcher for remote and local models
- **`launch()`** library entry point — embed the TUI in your own binary with custom `AgentOptions`
- **Streaming rendering** with markdown + syntect syntax highlighting in code blocks
- **Tool panel** — live view of in-flight and completed tool calls with approval prompts
- **Session persistence** — JSONL history via `swink-agent-memory`; resume any prior session from the picker
- **Credential wizard** (`wizard` module) — interactive provider setup backed by `keyring`
- **`local` feature** — bundles `swink-agent-local-llm` so the TUI can run offline on SmolLM3-3B
- **Mouse support, copy via `arboard`, and configurable key bindings**

## Quick Start

Install and run the standalone binary:

```sh
cargo install swink-agent-tui
swink
```

Or embed the TUI in your own crate:

```toml
[dependencies]
swink-agent = "0.9.0"
swink-agent-adapters = { version = "0.9.0", features = ["anthropic", "openai"] }
swink-agent-tui = "0.9.0"
tokio = { version = "1", features = ["full"] }
dotenvy = "0.15"
```

```rust,ignore
use swink_agent::prelude::*;
use swink_agent_adapters::build_remote_connection_for_model;
use swink_agent_tui::{TuiConfig, launch, restore_terminal, setup_terminal};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    let connections = ModelConnections::builder()
        .primary(build_remote_connection_for_model("claude-sonnet-4-6")?)
        .fallback(build_remote_connection_for_model("gpt-4.1")?)
        .build();

    let options = AgentOptions::from_connections("You are a helpful assistant.", connections)
        .with_default_tools();

    let mut terminal = setup_terminal()?;
    let result = launch(TuiConfig::default(), &mut terminal, options).await;
    restore_terminal()?;
    result
}
```

## Architecture

`App` owns the TUI state machine: a `ratatui` rendering loop on one task, a `crossterm` event stream on another, and an `Agent` driving the LLM on a third — coordinated through `tokio::sync::mpsc` channels. Tool-approval requests cross the channel as `ToolApprovalRequest`, and the user's response flows back via a `oneshot` so the agent loop blocks on exactly the decision it needs. Session storage reuses the `JsonlSessionStore` from `swink-agent-memory`, so on-disk history is interchangeable with any other `swink-agent` consumer.

No `unsafe` code (`#![forbid(unsafe_code)]`). Credentials entered through the wizard are stored in the OS keyring (`keyring` crate) — never in plaintext config.

---

Part of the [swink-agent](https://github.com/SuperSwinkAI/Swink-Agent) workspace — see the [main README](https://github.com/SuperSwinkAI/Swink-Agent#readme) for workspace overview and setup.
