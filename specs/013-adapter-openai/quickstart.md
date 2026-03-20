# Quickstart: Adapter: OpenAI

**Feature**: 013-adapter-openai | **Date**: 2026-03-20

## Prerequisites

- Rust 1.88+ (edition 2024)
- `swink-agent` core crate available as a path dependency
- OpenAI API key (set `OPENAI_API_KEY` environment variable for live tests)

## Build & Test

```bash
# Build the adapters crate
cargo build -p swink-agent-adapters

# Run all adapter unit tests
cargo test -p swink-agent-adapters

# Run only OpenAI-specific tests
cargo test -p swink-agent-adapters openai

# Run live integration tests (requires OPENAI_API_KEY)
cargo test -p swink-agent-adapters --test openai_live -- --ignored

# Full workspace check
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

## Usage Example

```rust
use swink_agent_adapters::OpenAiStreamFn;
use swink_agent::stream::StreamFn;

// Create the adapter -- works with any OpenAI-compatible endpoint
let stream_fn = OpenAiStreamFn::new(
    "https://api.openai.com",
    std::env::var("OPENAI_API_KEY").unwrap(),
);

// Use with the agent loop
let agent = Agent::builder()
    .model_spec(model_spec)
    .stream_fn(stream_fn)
    .build();
```

## Alternative Provider Example

```rust
// Local vLLM server
let stream_fn = OpenAiStreamFn::new(
    "http://localhost:8000",
    "not-needed",  // Most local servers ignore the key
);

// Groq
let stream_fn = OpenAiStreamFn::new(
    "https://api.groq.com/openai",
    std::env::var("GROQ_API_KEY").unwrap(),
);

// Together AI
let stream_fn = OpenAiStreamFn::new(
    "https://api.together.xyz",
    std::env::var("TOGETHER_API_KEY").unwrap(),
);
```

## Key Files

| File | Purpose |
|------|---------|
| `adapters/src/openai.rs` | `OpenAiStreamFn`, SSE stream parsing, tool call delta processing |
| `adapters/src/openai_compat.rs` | `OaiChatRequest`, `OaiChunk`, `OaiConverter` -- shared request/response types |
| `adapters/src/lib.rs` | Crate root, re-exports `OpenAiStreamFn` |
| `adapters/src/base.rs` | `AdapterBase` shared HTTP client |
| `adapters/src/convert.rs` | `MessageConverter` trait, `convert_messages` |
| `adapters/src/finalize.rs` | `StreamFinalize` trait for clean block closure |
| `adapters/src/sse.rs` | `SseStreamParser`, `sse_data_lines` shared SSE parser |
| `adapters/tests/openai_live.rs` | Live integration tests |
