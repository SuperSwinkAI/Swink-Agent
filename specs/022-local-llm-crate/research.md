# Research: Local LLM Crate

**Branch**: `022-local-llm-crate` | **Date**: 2026-03-20

## Design Decisions

### 1. mistral.rs for GGUF Inference

**Decision**: Use `mistralrs` (0.7) as the inference engine for loading and running quantized GGUF models.

**Rationale**: mistral.rs provides a mature, actively maintained Rust-native inference engine with GGUF support, streaming token generation, and tool-call handling. It handles quantization, KV-cache management, and hardware-specific optimizations internally. Wrapping it avoids rebuilding inference from scratch.

**Alternatives Rejected**:
- **llama.cpp bindings (llama-cpp-rs)**: C++ dependency with FFI boundary; mistral.rs is pure Rust and aligns with the `#[forbid(unsafe_code)]` policy at the crate boundary (mistral.rs handles its own internals).
- **candle**: Lower-level tensor library requiring manual model architecture implementation; mistral.rs provides ready-to-use model runners.
- **Custom GGUF parser**: Massive engineering effort with no benefit over established libraries.

### 2. SmolLM3-3B as Default Inference Model

**Decision**: Use SmolLM3-3B (quantized Q4_K_M, ~2.1 GB) as the default local model for text generation and tool use.

**Rationale**: SmolLM3-3B is optimized for on-device inference on consumer hardware (8GB+ RAM). It supports tool calling natively, fits within the project's target of offline-capable agents on developer machines, and balances output quality against resource constraints. The Q4_K_M quantization provides a good quality/size trade-off.

**Alternatives Rejected**:
- **Llama 3.2 3B**: Larger download, comparable quality at this parameter count.
- **Phi-3 mini**: Good quality but less consistent tool-calling support.
- **Gemma 2B**: Smaller but noticeably lower quality for agentic tasks.

### 3. EmbeddingGemma-300M for Embeddings

**Decision**: Use EmbeddingGemma-300M as the default local embedding model.

**Rationale**: At 300M parameters, it is compact enough for fast local vectorization while producing quality embeddings suitable for similarity search and retrieval. The small size means it loads quickly alongside the inference model without excessive memory pressure.

**Alternatives Rejected**:
- **all-MiniLM-L6-v2**: Older architecture; lower embedding quality on modern benchmarks.
- **Nomic Embed**: Larger model; unnecessary for local developer use cases.
- **BGE-small**: Comparable but less ecosystem support in mistral.rs.

### 4. Lazy Download with HuggingFace Verification

**Decision**: Models are downloaded lazily on first `ensure_ready()` call using `hf-hub`. Integrity verification (ETag/SHA) is delegated to `hf-hub`'s built-in mechanisms. No separate checksum step.

**Rationale**: Lazy download means the crate adds zero overhead when not used — no model files are fetched until inference is actually requested. `hf-hub` already verifies file integrity during download and caches in `~/.cache/huggingface/hub/`. Re-implementing verification would duplicate existing, well-tested logic.

**Alternatives Rejected**:
- **Eager download at crate initialization**: Downloads multi-GB files even if inference is never used; wastes bandwidth and time.
- **Manual SHA256 verification after download**: Duplicates `hf-hub` built-in verification; adds maintenance burden for checksum updates.
- **Bundled model weights in the crate**: Unreasonable crate size (2+ GB); violates crate distribution norms.

### 5. Silent Truncation for Inference Context Overflow

**Decision**: When input messages exceed the local model's context window (8192 tokens by default), silently truncate to fit by keeping the most recent messages.

**Rationale**: Local models have strict context limits (NoPE architecture caps at 8192). Erroring would break the agent loop for normal conversations that happen to be long. Silent truncation matches the core crate's existing sliding window behavior — the local model just has a smaller window. The most recent messages are kept because they contain the current task context.

**Alternatives Rejected**:
- **Error on overflow**: Breaks agent loop for routine long conversations; poor developer experience.
- **Summarize-then-truncate**: Requires an extra inference pass; adds latency and complexity.
- **Keep oldest messages**: Loses current context; agent generates irrelevant responses.

### 6. Error for Embedding Max Length Overflow

**Decision**: When text input exceeds the embedding model's maximum input length, return an explicit error rather than silently truncating.

**Rationale**: Unlike inference (where truncation preserves a usable conversation tail), truncating embedding input silently produces vectors that do not represent the full input — this corrupts similarity comparisons without the caller knowing. An explicit error lets the caller decide how to chunk or truncate their input.

**Alternatives Rejected**:
- **Silent truncation**: Produces misleading embeddings; similarity scores become unreliable without any signal to the caller.
- **Automatic chunking with mean pooling**: Adds complexity and changes the embedding semantics; should be the caller's decision.

## Open Questions

None — all clarifications resolved in the spec.
