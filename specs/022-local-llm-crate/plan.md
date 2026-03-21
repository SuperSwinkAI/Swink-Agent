# Implementation Plan: Local LLM Crate

**Branch**: `022-local-llm-crate` | **Date**: 2026-03-20 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/022-local-llm-crate/spec.md`

## Summary

Implement the `swink-agent-local-llm` workspace crate providing on-device LLM inference using mistral.rs for GGUF model loading. The crate defines `LocalModel` for text generation with quantized weights, `LocalStreamFn` implementing the standard `StreamFn` interface so local models are interchangeable with cloud providers, `EmbeddingModel` for local text vectorization, automatic message conversion to local model format, model presets (SmolLM3-3B for inference, EmbeddingGemma-300M for embeddings), lazy download with HuggingFace Hub caching, and `ProgressCallbackFn` for download/load progress reporting.

## Technical Context

**Language/Version**: Rust 1.88 (edition 2024)
**Primary Dependencies**: `mistralrs` (0.7, GGUF inference engine), `hf-hub` (HuggingFace model download with ETag/SHA verification), `tokio`, `tokio-stream`, `futures`, `serde`/`serde_json`, `thiserror`, `tracing`, `uuid`
**Storage**: Model weights cached in `~/.cache/huggingface/hub/` (managed by `hf-hub`)
**Testing**: `cargo test -p swink-agent-local-llm`; live tests (`--ignored`) for real inference requiring ~2.1 GB download
**Target Platform**: Cross-platform library crate (Linux, macOS, Windows); consumer hardware with 8GB+ RAM
**Project Type**: Library crate (`swink-agent-local-llm`) within the `swink-agent` workspace
**Performance Goals**: Streaming token delivery; lazy model loading; zero-cost when not used
**Constraints**: No unsafe code; context capped at 8192 tokens (NoPE architecture, overridable via `LOCAL_CONTEXT_LENGTH` env var); single-process assumption; `StreamFn` interface compatibility
**Scale/Scope**: Single-user local inference; quantized 4-bit weights for quality/resource balance

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| # | Principle | Status | Notes |
|---|-----------|--------|-------|
| I | Library-First | PASS | `swink-agent-local-llm` is its own workspace crate isolating heavy native dependencies (mistral.rs) from core. Depends on `swink-agent` via path; no reverse dependency. |
| II | Test-Driven Development | PASS | Unit tests for message conversion, presets, error variants, progress callbacks. Live integration tests (`--ignored`) exercise real inference and embedding with downloaded models. |
| III | Efficiency & Performance | PASS | Lazy model download avoids unnecessary work. Streaming token delivery minimizes latency to first token. Context capped at 8192 tokens to match model architecture. |
| IV | Leverage the Ecosystem | PASS | Uses `mistralrs` for GGUF inference (not hand-rolled), `hf-hub` for model download/caching with built-in integrity verification. No custom download or model loading code. |
| V | Provider Agnosticism | PASS | `LocalStreamFn` implements the standard `StreamFn` interface. The agent loop treats local models identically to cloud providers. No provider-specific types leak into core. |
| VI | Safety & Correctness | PASS | `#[forbid(unsafe_code)]`. Model download/load errors produce `LocalModelError` variants with clear messages. Silent truncation for context overflow; explicit error for embedding length overflow. Cost is always zero. |

## Project Structure

### Documentation (this feature)

```text
specs/022-local-llm-crate/
├── plan.md              # This file
├── research.md          # Design decisions and trade-offs
├── data-model.md        # Entity definitions and relationships
├── quickstart.md        # Getting started guide
├── contracts/
│   └── public-api.md    # Public API surface contract
└── tasks.md             # Phase 2 output (created by /speckit.tasks)
```

### Source Code (repository root)

```text
local-llm/
├── Cargo.toml
├── CLAUDE.md
└── src/
    ├── lib.rs            # Re-exports public API
    ├── model.rs          # LocalModel — model lifecycle (Unloaded → Downloading → Loading → Ready)
    ├── stream.rs         # LocalStreamFn — StreamFn implementation bridging local inference to agent loop
    ├── embedding.rs      # EmbeddingModel — text-to-vector embedding
    ├── convert.rs        # Message conversion from LlmMessage to local model format
    ├── preset.rs         # ModelPreset — named configuration bundles (SmolLM3-3B, EmbeddingGemma-300M)
    ├── progress.rs       # ProgressCallbackFn — download/load progress reporting
    └── error.rs          # LocalModelError — Download, Loading, Inference, Embedding variants

local-llm/tests/
├── common/
│   └── mod.rs            # Shared test helpers
├── local_live.rs         # Live inference tests (--ignored, requires model download)
└── embedding_live.rs     # Live embedding tests (--ignored, requires model download)
```

**Structure Decision**: The `local-llm/` crate already exists as a workspace member with all source files in place. The one-concern-per-file convention is followed. `lib.rs` re-exports all public types.

## Complexity Tracking

No constitution violations. No complexity justifications required.
