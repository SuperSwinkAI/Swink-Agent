# Quickstart: Workspace & Cargo Scaffold

**Feature**: 001-workspace-scaffold

## Prerequisites

- Rust toolchain (rustup installed — the project's `rust-toolchain.toml`
  will auto-select Rust 1.88)
- Git

## Build

```bash
# Clone and build the entire workspace
git clone https://github.com/SuperSwinkAI/Swink-Agent.git
cd Swink-Agent
cargo build --workspace
```

## Verify

```bash
# All crates compile with zero warnings
cargo clippy --workspace -- -D warnings

# Core crate compiles with and without default features
cargo build -p swink-agent
cargo build -p swink-agent --no-default-features

# Each crate builds individually
cargo build -p swink-agent-adapters
cargo build -p swink-agent-memory
cargo build -p swink-agent-local-llm
cargo build -p swink-agent-eval
cargo build -p swink-agent-tui
cargo build -p xtask

# Run tests
cargo test --workspace

# Verify formatter is deterministic
cargo fmt --check
```

## Use as a dependency

```toml
# In your Cargo.toml
[dependencies]
swink-agent = { git = "https://github.com/SuperSwinkAI/Swink-Agent.git" }
```

```rust
// In your code
use swink_agent::*;
```

## Scaffold contents

After this feature, each crate contains only structural scaffolding:

- `lib.rs` with `#![forbid(unsafe_code)]` and stub re-exports
- `main.rs` (TUI and xtask) with minimal entry points
- `Cargo.toml` with correct dependencies using workspace inheritance
- No business logic — that comes in features 002+
