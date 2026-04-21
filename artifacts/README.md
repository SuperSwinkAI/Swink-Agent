# swink-agent-artifacts

[![Crates.io](https://img.shields.io/crates/v/swink-agent-artifacts.svg)](https://crates.io/crates/swink-agent-artifacts)
[![Docs.rs](https://docs.rs/swink-agent-artifacts/badge.svg)](https://docs.rs/swink-agent-artifacts)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](https://github.com/SuperSwinkAI/Swink-Agent/blob/main/LICENSE-MIT)

Versioned artifact storage backends for [`swink-agent`](https://crates.io/crates/swink-agent) sessions — capture, version, and retrieve the blobs a tool run produces.

## Features

- **`FileArtifactStore`** — persistent on-disk store with atomic writes and an append-only version log
- **`InMemoryArtifactStore`** — zero-dependency backend for tests and ephemeral runs
- Streaming reads and writes (`bytes::Bytes`) — no whole-file copies
- Name validation (`validate_artifact_name`) enforces safe, portable keys
- Implements the `swink_agent::ArtifactStore` trait behind the core crate's `artifact-store` feature gate — drop either backend directly into `AgentOptions`

## Quick Start

```toml
[dependencies]
swink-agent = { version = "0.8", features = ["artifact-store"] }
swink-agent-artifacts = "0.8"
tokio = { version = "1", features = ["full"] }
```

```rust
use std::sync::Arc;
use swink_agent_artifacts::FileArtifactStore;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // `try_new` surfaces I/O errors; `new` would panic on a bad path.
    let store = Arc::new(FileArtifactStore::try_new("./artifacts")?);

    // Pass `store` into AgentOptions via the artifact-store integration;
    // tools can then write versioned artifacts during a turn.
    // Each write produces a new immutable version; reads can target latest or a specific version.
    Ok(())
}
```

## Architecture

The crate is a pair of trait implementations — `FileArtifactStore` layers a directory-per-artifact structure with a JSONL version log on top of `tokio::fs`, and `InMemoryArtifactStore` keeps the same semantics in a `HashMap`. Both use `bytes::Bytes` for zero-copy payload handoff and stream through `futures::Stream` so large artifacts never load fully into memory.

No `unsafe` code (`#![forbid(unsafe_code)]`). Name validation rejects path traversal and reserved characters before any filesystem call.

---

Part of the [swink-agent](https://github.com/SuperSwinkAI/Swink-Agent) workspace — see the [main README](https://github.com/SuperSwinkAI/Swink-Agent#readme) for workspace overview and setup.
