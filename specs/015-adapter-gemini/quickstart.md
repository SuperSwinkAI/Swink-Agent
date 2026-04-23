# Quickstart: Adapter: Google Gemini

**Feature**: 015-adapter-gemini | **Date**: 2026-03-24

## Prerequisites

- Rust latest stable (edition 2024)
- `swink-agent` core crate available as a path dependency
- Google Gemini API key (set `GEMINI_API_KEY` environment variable for live tests)

## Build & Test

```bash
# Build the adapters crate
cargo build -p swink-agent-adapters

# Run all adapter unit tests
cargo test -p swink-agent-adapters

# Run only Gemini-specific tests
cargo test -p swink-agent-adapters google

# Run live integration tests (requires GEMINI_API_KEY)
cargo test -p swink-agent-adapters --test google_live -- --ignored

# Full workspace check
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

## Usage Example

```rust
use swink_agent_adapters::GeminiStreamFn;
use swink_agent::{ApiVersion, StreamFn};

// Create the adapter
let stream_fn = GeminiStreamFn::new(
    "https://generativelanguage.googleapis.com",
    std::env::var("GEMINI_API_KEY").unwrap(),
    ApiVersion::V1beta,
);

// Use with the agent loop
let agent = Agent::builder()
    .model_spec(ModelSpec::new("google", "gemini-2.5-flash"))
    .stream_fn(stream_fn)
    .build();
```

## API Version Selection

```rust
use swink_agent::ApiVersion;

// Stable API (limited feature set)
let stream_fn = GeminiStreamFn::new(url, key, ApiVersion::V1);

// Beta API (thinking support, latest features)
let stream_fn = GeminiStreamFn::new(url, key, ApiVersion::V1beta);
```

## Key Files

| File | Purpose |
|------|---------|
| `adapters/src/google.rs` | `GeminiStreamFn`, message conversion, SSE parsing, thinking support |
| `adapters/src/lib.rs` | Crate root, re-exports `GeminiStreamFn` |
| `adapters/src/classify.rs` | Shared error classifier used for HTTP errors |
| `adapters/src/convert.rs` | `extract_tool_schemas` used for function declarations |
| `adapters/src/finalize.rs` | `StreamFinalize` trait for clean block closure |
| `adapters/src/sse.rs` | `sse_data_lines` SSE parser |
| `adapters/tests/google.rs` | Wiremock unit tests |
| `adapters/tests/google_live.rs` | Live integration tests |
