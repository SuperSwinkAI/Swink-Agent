# Quickstart: xAI Adapter

**Feature**: 017-adapter-xai | **Date**: 2026-04-02

## Prerequisites

- Rust latest stable (edition 2024)
- xAI API key (set `XAI_API_KEY` env var or pass directly)

## Add Dependency

```toml
[dependencies]
swink-agent-adapters = { path = "../adapters", features = ["xai"] }
```

## Basic Usage

```rust
use swink_agent_adapters::XAiStreamFn;
use swink_agent::types::ModelSpec;

// Create the adapter
let stream_fn = XAiStreamFn::new(
    "https://api.x.ai",
    std::env::var("XAI_API_KEY").unwrap(),
);

// Use with an Agent
let agent = Agent::builder()
    .model(ModelSpec::new("grok-4-1-fast-non-reasoning"))
    .stream_fn(stream_fn)
    .build();
```

## Using Model Catalog Presets

```rust
use swink_agent::model_catalog::ModelCatalog;
use swink_agent_adapters::build_remote_connection;

let catalog = ModelCatalog::load();
let preset = catalog.preset("grok_4_1_fast_non_reasoning").unwrap();
let (stream_fn, model_spec) = build_remote_connection(&preset, None)?;
```

## Run Tests

```bash
# Unit tests (no API key needed)
cargo test -p swink-agent-adapters --features xai

# Live tests (requires XAI_API_KEY)
cargo test -p swink-agent-adapters --test xai_live -- --ignored
```

## Available Models

| Preset ID | Model | Best For |
|-----------|-------|----------|
| `grok_4_20_reasoning` | Grok 4.20 Reasoning | Complex reasoning tasks |
| `grok_4_20_non_reasoning` | Grok 4.20 | General-purpose, high quality |
| `grok_4_1_fast_reasoning` | Grok 4.1 Fast Reasoning | Fast agentic tool calling |
| `grok_4_1_fast_non_reasoning` | Grok 4.1 Fast | Fast, cheap general use |
| `grok_4_20_multi_agent` | Grok 4.20 Multi-Agent | Multi-agent workflows |
