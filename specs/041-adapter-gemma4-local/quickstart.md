# Quickstart: Gemma 4 Local Default (Direct Inference)

**Feature**: 041-adapter-gemma4-local  
**Date**: 2026-04-04

## Prerequisites

- Rust 1.88+ with edition 2024
- `mistralrs` 0.8+ (blocked until NaN logits bug #2051 is fixed)
- ~4 GB free disk space (for E2B Q4_K_M download + cache)
- ~6 GB RAM recommended for E2B inference

## Quick Start

### Using Gemma 4 E2B as default (after feature is stable)

```rust
// In Cargo.toml:
// swink-agent-local-llm = { version = "0.6", features = ["gemma4"] }

use swink_agent_local_llm::default_local_connection;

// Automatically uses Gemma 4 E2B (128K context)
// Downloads ~3.5 GB on first use
let connection = default_local_connection()?;
```

### Overriding the default model

```bash
# Use SmolLM3-3B instead of Gemma 4 E2B
export LOCAL_MODEL_REPO="bartowski/SmolLM3-3B-GGUF"
export LOCAL_MODEL_FILE="SmolLM3-3B-Q4_K_M.gguf"
export LOCAL_CONTEXT_LENGTH=8192
```

### Using a specific Gemma 4 variant

```rust
use swink_agent_local_llm::{ModelPreset, LocalModel, ModelConfig};

// Use the larger 26B variant (requires ~20 GB RAM)
let config = ModelPreset::Gemma4_26B.config();
let model = LocalModel::new(config);
```

### Using without the `gemma4` feature

```rust
// Without the feature, SmolLM3-3B remains the default
// swink-agent-local-llm = "0.6"  (no gemma4 feature)

use swink_agent_local_llm::default_local_connection;
let connection = default_local_connection()?; // SmolLM3-3B
```

## Build & Test

```bash
# Build with Gemma 4 support
cargo build -p swink-agent-local-llm --features gemma4

# Run unit tests (no model download needed)
cargo test -p swink-agent-local-llm --features gemma4

# Run live tests (downloads ~3.5 GB on first run)
cargo test -p swink-agent-local-llm --features gemma4 --test local_live -- --ignored

# Build with Metal acceleration (macOS)
cargo build -p swink-agent-local-llm --features "gemma4,metal"
```

## Alternative Backends (no code changes)

If you prefer not to use the local-llm crate, Gemma 4 E2B works with the existing OpenAI-compatible adapter via external servers:

### Ollama (recommended)
```bash
ollama run gemma4:e2b
# Then use the Ollama adapter as normal
```

### llama.cpp server
```bash
llama-server -m gemma-4-E2B-it-Q4_K_M.gguf \
    --chat-template-kwargs '{"enable_thinking":true}' --port 8080
# Point OpenAI adapter at http://localhost:8080/v1
```

### vLLM
```bash
vllm serve google/gemma-4-E2B-it --enable-auto-tool-choice \
    --reasoning-parser gemma4 --tool-call-parser gemma4
# Point OpenAI adapter at http://localhost:8000/v1
```
