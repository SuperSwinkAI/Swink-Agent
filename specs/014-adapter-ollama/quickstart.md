# Quickstart: Adapter: Ollama

**Feature**: 014-adapter-ollama | **Date**: 2026-03-20

## Prerequisites

- Rust latest stable (edition 2024)
- `swink-agent` core crate available as a path dependency
- Ollama installed and running (`ollama serve` or the Ollama desktop app)
- A model pulled locally (e.g. `ollama pull llama3.2`)

## Build & Test

```bash
# Build the adapters crate
cargo build -p swink-agent-adapters

# Run all adapter unit tests
cargo test -p swink-agent-adapters

# Run only Ollama-specific tests
cargo test -p swink-agent-adapters ollama

# Run live integration tests (requires running Ollama instance)
cargo test -p swink-agent-adapters --test ollama_live -- --ignored

# Full workspace check
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

## Usage Example

```rust
use swink_agent_adapters::OllamaStreamFn;
use swink_agent::stream::StreamFn;

// Create the adapter -- defaults to local Ollama instance
let stream_fn = OllamaStreamFn::new("http://localhost:11434");

// Use with the agent loop
let agent = Agent::builder()
    .model_spec(model_spec)
    .stream_fn(stream_fn)
    .build();
```

## Remote Ollama Example

```rust
// Remote Ollama instance on a server
let stream_fn = OllamaStreamFn::new("http://my-gpu-server:11434");
```

## Key Files

| File | Purpose |
|------|---------|
| `adapters/src/ollama.rs` | `OllamaStreamFn`, NDJSON stream parsing, tool call processing |
| `adapters/src/lib.rs` | Crate root, re-exports `OllamaStreamFn` |
| `adapters/src/convert.rs` | `MessageConverter` trait, `convert_messages` |
| `adapters/src/finalize.rs` | `StreamFinalize` trait for clean block closure |
| `adapters/tests/ollama_live.rs` | Live integration tests |
