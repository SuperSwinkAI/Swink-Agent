# CLAUDE.md — Local Inference

## Scope

`local-llm/` — On-device inference via mistral.rs. SmolLM3-3B (text/tools) and EmbeddingGemma-300M (embeddings).

## Key Facts

- Models lazily downloaded from HuggingFace on first `ensure_ready()`. Cached in `~/.cache/huggingface/hub/`.
- `ModelState` lifecycle: `Unloaded → Downloading → Loading → Ready` (or `Failed`).
- Internal state (`InternalModelState`) holds the runner; public `ModelState` is a simple enum without the runner.
- Currently non-streaming (`send_chat_request` wrapped into event protocol). Future: switch to `stream_chat_request`.
- Cost is always zero.
- `ModelPreset` enum provides `SmolLM3_3B` and `EmbeddingGemma300M` for zero-config model setup.
- `ProgressEvent` enum: `DownloadProgress`, `DownloadComplete`, `LoadingProgress`, `LoadingComplete`.

## Lessons Learned

- **SmolLM3 `<think>` tags** — parsed via simple string matching (not regex) and routed to `ThinkingStart/Delta/End` events.
- **Context capped at 8192 tokens** (NoPE architecture). Override via `LOCAL_CONTEXT_LENGTH` env var.
- **mistralrs version pin** — API is actively evolving; pin to specific minor version.
- **`with_progress` returns `Result`** — call before cloning the `Arc`.
- **`ModelState` split** — public `ModelState` (re-exported from lib.rs) vs internal `InternalModelState` (holds `mistralrs::Model` runner). Stream code uses `InternalModelState`.
- **Embedding method naming** — `embed(text)` for single text, `embed_batch(texts)` for batch. Errors use `LocalModelError::Embedding` variant.

## Design Decisions

### Intentional duplication between `model.rs` and `embedding.rs`

Both modules follow the same `Arc<Inner>` + state-machine pattern (`Unloaded -> Downloading -> Loading -> Ready | Failed`), `RwLock`-guarded state, `Notify`-based readiness signalling, and progress callbacks. This structural similarity is deliberate rather than extracted into a generic type because:

- **Different runner types** — `model.rs` uses `mistralrs::GgufModelBuilder` for chat completion pipelines; `embedding.rs` uses `mistralrs::EmbeddingModelBuilder` for vectorization pipelines. These are distinct mistral.rs types with incompatible builder and inference APIs.
- **Different public APIs** — `LocalModel` exposes `runner()` for streaming chat completion; `EmbeddingModel` exposes `embed(text)` and `embed_batch(texts)` returning `Vec<f32>`.
- **Complexity trade-off** — A generic `LazyModel<Config, State>` abstraction would require type parameters threading through config, state, runner, and builder types. With only two implementations this adds indirection without meaningful deduplication.

## Build & Test

```bash
cargo build -p swink-agent-local-llm
cargo test -p swink-agent-local-llm
cargo clippy -p swink-agent-local-llm -- -D warnings
```

## Live Tests

```bash
cargo test -p swink-agent-local-llm --test local_live -- --ignored
cargo test -p swink-agent-local-llm --test embedding_live -- --ignored
```

Downloads ~2.1 GB on first run. Embedding model requires `HF_TOKEN` (gated).
