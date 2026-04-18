# Quickstart: 033 Workspace Feature Gates

**Date**: 2026-03-25

## What This Changes

Adds granular Cargo feature flags to the adapters and local-llm crates so consumers compile only the providers and backends they need. The root crate remains independent; consumers opt into feature-gated functionality through the relevant sub-crates.

## Implementation Order

1. **Adapters crate** — Add 9 feature flags + `all` + `default`. Gate `mod` + `pub use` in lib.rs. Gate `eventsource-stream` behind `proxy`, `sha2` behind `bedrock`.
2. **Local-LLM crate** — Add backend features (`metal`, `cuda`, `vulkan`) forwarding to llama-cpp-2.
3. **Verification** — `cargo test --workspace` with defaults. Minimal feature builds. CI matrix additions.

## Key Pattern (from policies crate)

```toml
# Cargo.toml
[features]
default = []
full = ["all"]
all = ["anthropic", "openai", ...]
anthropic = []
proxy = ["dep:eventsource-stream"]

[dependencies]
eventsource-stream = { version = "0.2", optional = true }
```

```rust
// lib.rs
#[cfg(feature = "anthropic")]
mod anthropic;
#[cfg(feature = "anthropic")]
pub use anthropic::AnthropicStreamFn;
```

## Verification Commands

```bash
# Default (all features) — must match current behavior
cargo test --workspace

# Single adapter
cargo build -p swink-agent-adapters --no-default-features --features anthropic

# No adapters (shared infra only)
cargo build -p swink-agent-adapters --no-default-features

# Local LLM with Metal
cargo build -p swink-agent-local-llm --features metal

# Bare minimum root
cargo build --no-default-features
```
