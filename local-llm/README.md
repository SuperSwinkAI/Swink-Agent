# swink-agent-local-llm

[![Crates.io](https://img.shields.io/crates/v/swink-agent-local-llm.svg)](https://crates.io/crates/swink-agent-local-llm)
[![Docs.rs](https://docs.rs/swink-agent-local-llm/badge.svg)](https://docs.rs/swink-agent-local-llm)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](https://github.com/SuperSwinkAI/Swink-Agent/blob/main/LICENSE-MIT)

On-device LLM inference for [`swink-agent`](https://crates.io/crates/swink-agent) powered by `llama.cpp` — ship an agent that runs with no network and no API keys.

## Features

- **SmolLM3-3B** (default, GGUF `Q4_K_M`, ~1.92 GB) — text generation, tool use, and reasoning on CPU-only hardware
- **Gemma 4 E2B** (`gemma4` feature, ~3.5 GB) — 128K context with native thinking mode and tool calling
- **EmbeddingGemma-300M** (<200 MB) — text embeddings for semantic search and RAG
- GGUF weights are lazily downloaded from HuggingFace on first use (`hf-hub`)
- GPU acceleration: `metal` (Apple), `cuda` (NVIDIA), `vulkan` (cross-platform) — CPU-only works by default
- `default_local_connection()` returns a ready `ModelConnection` — drop it into `ModelConnections` alongside remote adapters
- Models are designed for `Arc<>` sharing across concurrent tasks

## Quick Start

```toml
[dependencies]
swink-agent = "0.10.0"
swink-agent-local-llm = { version = "0.10.0", features = ["metal"] }  # or "cuda", "vulkan", or none for CPU
tokio = { version = "1", features = ["full"] }
```

```rust,ignore
use swink_agent::prelude::*;
use swink_agent_local_llm::default_local_connection;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let connections = ModelConnections::builder()
        .primary(default_local_connection()?)
        .build();

    let options = AgentOptions::from_connections(
        "You are a helpful assistant.",
        connections,
    );

    let mut agent = Agent::new(options);
    let result = agent.prompt_text("Explain GGUF in one sentence.").await?;
    println!("{}", result.assistant_text());
    Ok(())
}
```

## Architecture

`LocalStreamFn` implements the core `StreamFn` trait by driving a loaded `LocalModel` through a token-by-token generation loop, emitting the same `AssistantMessageEvent` stream as remote adapters. `ModelPreset` holds the catalog of supported GGUF weights and download URLs; `EmbeddingModel` runs a separate llama.cpp context tuned for pooled-output embeddings. First-run downloads show progress via a `ProgressCallbackFn` hook.

`swink-agent-local-llm` depends on `llama-cpp-2`, which builds a C++ backend via `cmake` and generates bindings via `bindgen`. Contributor machines need LLVM/libclang available; set `LIBCLANG_PATH` to the LLVM `bin` directory if auto-discovery fails. Expect ~5 minutes on the first build.

No `unsafe` code in this crate (`#![forbid(unsafe_code)]`). The `unsafe` required for FFI into `llama.cpp` is encapsulated in the upstream `llama-cpp-2` sys crate.

---

Part of the [swink-agent](https://github.com/SuperSwinkAI/Swink-Agent) workspace — see the [main README](https://github.com/SuperSwinkAI/Swink-Agent#readme) for workspace overview and setup.
