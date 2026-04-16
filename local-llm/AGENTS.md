# AGENTS.md — Local Inference

## Scope

`local-llm/` — On-device inference via mistral.rs. SmolLM3-3B (default), Gemma 4 E2B (opt-in, `gemma4` feature), and EmbeddingGemma-300M (embeddings).

## Key Facts

- Models lazily downloaded from HuggingFace on first `ensure_ready()`. Cached in `~/.cache/huggingface/hub/`.
- `ModelState` lifecycle: `Unloaded → Downloading → Loading → Ready` (or `Failed`).
- Internal state (`InternalModelState`) holds the runner; public `ModelState` is a simple enum without the runner.
- Uses `stream_chat_request` for true token-by-token streaming. Cost is always zero.
- `ModelPreset` enum provides `SmolLM3_3B`, `EmbeddingGemma300M`, and (with `gemma4` feature) `Gemma4E2B`, `Gemma4E4B`, `Gemma4_26B`, `Gemma4_31B`.
- `ProgressEvent` enum: `DownloadProgress`, `DownloadComplete`, `LoadingProgress`, `LoadingComplete`.
- **Default remains SmolLM3-3B** — `DEFAULT_LOCAL_PRESET_ID = "smollm3_3b"` unconditionally. Gemma 4 is opt-in via `ModelPreset::Gemma4E2B`.

## Lessons Learned

- **SmolLM3 `<think>` tags** — parsed via simple string matching (not regex) and routed to `ThinkingStart/Delta/End` events.
- **Context capped at 8192 tokens** (NoPE architecture). Override via `LOCAL_CONTEXT_LENGTH` env var.
- **mistralrs version pin** — API is actively evolving; pin to specific minor version.
- **SmolLM3 GGUF must fail fast on current `mistralrs`** — `mistralrs` 0.8.1 does not recognize the `smollm3` GGUF architecture. Reject `SmolLM3` configs before download/build with a typed `LocalModelError::Loading` instead of letting the dependency panic on `ensure_ready()`. The follow-up fix for a working default local model is separate work.
- **`with_progress` returns `Result`** — call before cloning the `Arc`.
- **`ModelState` split** — public `ModelState` (re-exported from lib.rs) vs internal `InternalModelState` (holds `mistralrs::Model` runner). Stream code uses `InternalModelState`.
- **Embedding method naming** — `embed(text)` for single text, `embed_batch(texts)` for batch. Errors use `LocalModelError::Embedding` variant.
- **Gemma 4 requires GPU** — CPU-only inference hangs silently on non-trivial prompts (BF16 safetensors on CPU is not viable). Build with `--features gemma4,cuda` (NVIDIA) or `--features gemma4,metal` (Apple Silicon). On Windows, `cl.exe` (MSVC) must be in PATH for the cuda feature to compile — use a VS 2022 Developer Command Prompt. A `tracing::warn!` is emitted at load time when Gemma 4 is used without any GPU feature compiled in.
- **Gemma 4 live tests are runtime-gated** — `local-llm/tests/common/mod.rs` uses `swink_agent::testing::should_run_test()` to detect OS/GPU support before calling `ensure_ready()`. This avoids hanging on unsupported hosts and keeps the skip reason close to the real constraint.
- **Metal builds need Apple's Metal toolchain** — on macOS, compiling with `--features metal` requires the `metal` compiler from Apple's Metal Toolchain. If the build script says `cannot execute tool 'metal'`, install it with `xcodebuild -downloadComponent MetalToolchain`.
- **Gemma 4 uses `MultimodalModelBuilder`** — `GgufModelBuilder` cannot load Gemma 4 (Per-Layer Embeddings architecture). `MultimodalModelBuilder::new(repo_id)` handles download internally, so the hf-hub download phase is skipped for Gemma 4.
- **Gemma 4 model family detection** — `ModelConfig::is_gemma4()` checks `repo_id` for `"gemma-4"` or `"gemma4"` substrings (behind `gemma4` feature flag).
- **Gemma 4 thinking mode** — `<|think|>\n` prepended to system prompt in `convert.rs` when `config.is_gemma4() && thinking_enabled`. mistralrs has no `think: true` API for direct inference.
- **Gemma 4 thinking output** — `ChannelThoughtParser` in `stream.rs` handles cross-chunk `<|channel>thought\n...<channel|>` delimiters. Stateful 4-state machine (Normal → PartialOpen → InThinking → PartialClose).
- **Gemma 4 tool calls** — `ToolCallParser` in `stream.rs` handles cross-chunk `<|tool_call>call:{name}{args}<tool_call|>` format. IDs are generated as UUIDs. `Gemma4LocalConverter` wraps tool results as `<|tool_result>{tool_call_id}\n{text}<tool_result|>`.
- **Gemma 4 partial delimiter matching must be UTF-8 safe** — when scanning for split `<|channel>thought\n` and `<tool_call|>` delimiters, only slice `&str` values at character boundaries. Reuse the shared suffix helper in `stream.rs`; raw byte-offset suffix slicing can panic on multibyte output.
- **Two converter types** — `LocalConverter` (SmolLM3) and `Gemma4LocalConverter` (Gemma 4) both implement `MessageConverter`. `convert_context_messages` dispatches based on `config.is_gemma4()`.
- **LazyLoader waiters must treat `Unloaded` as terminal for the current attempt** — `wait_until_ready()` now returns on `Unloaded`/`Failed` as well as `Ready`, and `ensure_ready()` re-checks loader state after every wakeup so an `unload()` during another caller's load does not strand waiters forever.

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

# With Gemma 4 support
cargo build -p swink-agent-local-llm --features gemma4
cargo test -p swink-agent-local-llm --features gemma4
cargo clippy -p swink-agent-local-llm --features gemma4 -- -D warnings

# Verify SmolLM3 still works without gemma4 feature
cargo build -p swink-agent-local-llm --no-default-features
```

## Live Tests

```bash
cargo test -p swink-agent-local-llm --test local_live -- --ignored
cargo test -p swink-agent-local-llm --test embedding_live -- --ignored

# Gemma 4 live tests (downloads ~5 GB on first run)
cargo test -p swink-agent-local-llm --features gemma4 --test local_live -- --ignored
```

SmolLM3 downloads ~1.92 GB on first run. Embedding model requires `HF_TOKEN` (gated). Gemma 4 E2B downloads ~5 GB safetensors on first run.

Gemma 4 live tests now short-circuit unless the host matches the compiled backend:
- `metal`: macOS on Apple Silicon, Metal-capable GPU, and Apple's Metal toolchain installed.
- `cuda` / `cudnn`: NVIDIA GPU detected on the host.
- no GPU backend feature: tests print a skip reason and return immediately.
