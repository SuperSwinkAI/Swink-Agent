# Quickstart: Adapter: Anthropic

**Feature**: 012-adapter-anthropic | **Date**: 2026-03-20

## Prerequisites

- Rust latest stable (edition 2024)
- `swink-agent` core crate available as a path dependency
- Anthropic API key (set `ANTHROPIC_API_KEY` environment variable for live tests)

## Build & Test

```bash
# Build the adapters crate
cargo build -p swink-agent-adapters

# Run all adapter unit tests
cargo test -p swink-agent-adapters

# Run only Anthropic-specific tests
cargo test -p swink-agent-adapters anthropic

# Run live integration tests (requires ANTHROPIC_API_KEY)
cargo test -p swink-agent-adapters --test anthropic_live -- --ignored

# Full workspace check
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

## Usage Example

```rust
use swink_agent_adapters::AnthropicStreamFn;
use swink_agent::stream::StreamFn;

// Create the adapter
let stream_fn = AnthropicStreamFn::new(
    "https://api.anthropic.com",
    std::env::var("ANTHROPIC_API_KEY").unwrap(),
);

// Use with the agent loop
let agent = Agent::builder()
    .model_spec(model_spec)
    .stream_fn(stream_fn)
    .build();
```

## Thinking Support

```rust
use swink_agent::types::{ModelSpec, ThinkingLevel};

// Enable thinking via ModelSpec
let model_spec = ModelSpec::builder()
    .model_id("claude-sonnet-4-6-20260320")
    .thinking_level(ThinkingLevel::Medium)  // 5000 token budget
    .build();

// The adapter automatically includes thinking configuration
// in the request and streams thinking blocks as separate events.
```

## Key Files

| File | Purpose |
|------|---------|
| `adapters/src/anthropic.rs` | `AnthropicStreamFn`, message conversion, SSE parsing, thinking support |
| `adapters/src/lib.rs` | Crate root, re-exports `AnthropicStreamFn` |
| `adapters/src/base.rs` | `AdapterBase` shared HTTP client |
| `adapters/src/convert.rs` | `extract_tool_schemas` used for tool definitions |
| `adapters/src/finalize.rs` | `StreamFinalize` trait for clean block closure |
| `adapters/tests/anthropic_live.rs` | Live integration tests |
