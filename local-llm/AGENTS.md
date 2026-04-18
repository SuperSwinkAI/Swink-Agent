# AGENTS.md — Local Inference

## Scope

`local-llm/` — On-device inference via llama.cpp (Rust bindings: `llama-cpp-2`). SmolLM3-3B (default), Gemma 4 E2B (opt-in, `gemma4` feature), and EmbeddingGemma-300M (embeddings). All models use GGUF format.

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
- **llama-cpp-2 version pin** — API is actively evolving; pin to specific minor version.
- **SmolLM3 is fully supported** — `llama-cpp-2` (llama.cpp) natively supports the SmolLM3 GGUF architecture. No fail-fast rejection needed.
- **`with_progress` returns `Result`** — call before cloning the `Arc`.
- **`ModelState` split** — public `ModelState` (re-exported from lib.rs) vs internal `InternalModelState` (holds the llama.cpp model handle). Stream code uses `InternalModelState`.
- **`LlamaContext` is `!Send`** — llama.cpp contexts cannot be sent across threads. Inference uses a dedicated thread pattern (`std::thread::spawn` + channel) to keep the context on a single OS thread.
- **Embedding method naming** — `embed(text)` for single text, `embed_batch(texts)` for batch. Errors use `LocalModelError::Embedding` variant.
- **Gemma 4 works on CPU** — unlike the previous mistralrs-based implementation which hung silently on CPU, llama.cpp handles Gemma 4 GGUF inference on CPU correctly (though slowly). GPU acceleration (`--features gemma4,metal` on Apple Silicon, `--features gemma4,cuda` on NVIDIA) is strongly recommended for usable performance.
- **Gemma 4 live tests are runtime-gated** — `local-llm/tests/common/mod.rs` uses `swink_agent::testing::should_run_test()` to detect OS/GPU support before calling `ensure_ready()`. This avoids hanging on unsupported hosts and keeps the skip reason close to the real constraint.
- **Metal builds need Apple's Metal toolchain** — on macOS, compiling with `--features metal` requires the `metal` compiler from Apple's Metal Toolchain. If the build script says `cannot execute tool 'metal'`, install it with `xcodebuild -downloadComponent MetalToolchain`.
- **Gemma 4 uses GGUF** — all models (including Gemma 4) now use GGUF format loaded via `LlamaModel::load_from_file`. Gemma 4 GGUF repos are from `bartowski/` (e.g., `bartowski/google_gemma-4-E2B-it-GGUF`).
- **Gemma 4 model family detection** — `ModelConfig::is_gemma4()` checks `repo_id` for `"gemma-4"` or `"gemma4"` substrings (behind `gemma4` feature flag).
- **Gemma 4 thinking mode** — `<|think|>\n` prepended to system prompt in `convert.rs` when `config.is_gemma4() && thinking_enabled`. llama.cpp has no `think: true` API for direct inference.
- **Gemma 4 thinking output** — `ChannelThoughtParser` in `stream.rs` handles cross-chunk `<|channel>thought\n...<channel|>` delimiters. Stateful 4-state machine (Normal → PartialOpen → InThinking → PartialClose).
- **Gemma 4 tool calls** — `ToolCallParser` in `stream.rs` handles cross-chunk `<|tool_call>call:{name}{args}<tool_call|>` format. IDs are generated as UUIDs. `Gemma4LocalConverter` wraps tool results as `<|tool_result>{tool_call_id}\n{text}<tool_result|>`.
- **Gemma 4 partial delimiter matching must be UTF-8 safe** — when scanning for split `<|channel>thought\n` and `<tool_call|>` delimiters, only slice `&str` values at character boundaries. Reuse the shared suffix helper in `stream.rs`; raw byte-offset suffix slicing can panic on multibyte output.
- **Two converter types** — `LocalConverter` (SmolLM3) and `Gemma4LocalConverter` (Gemma 4) both implement `MessageConverter`. `convert_context_messages` dispatches based on `config.is_gemma4()`.
- **LazyLoader waiters must treat `Unloaded` as terminal for the current attempt** — `wait_until_ready()` now returns on `Unloaded`/`Failed` as well as `Ready`, and `ensure_ready()` re-checks loader state after every wakeup so an `unload()` during another caller's load does not strand waiters forever.

## Design Decisions

### Intentional duplication between `model.rs` and `embedding.rs`

Both modules follow the same `Arc<Inner>` + state-machine pattern (`Unloaded -> Downloading -> Loading -> Ready | Failed`), `RwLock`-guarded state, `Notify`-based readiness signalling, and progress callbacks. This structural similarity is deliberate rather than extracted into a generic type because:

- **Different runner types** — `model.rs` uses `LlamaModel::load_from_file` for chat completion pipelines; `embedding.rs` uses the same loader for vectorization pipelines. Both use GGUF format but have distinct inference APIs.
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

SmolLM3 downloads ~1.92 GB on first run. Embedding model downloads GGUF from `unsloth/embeddinggemma-300m-GGUF`. Gemma 4 E2B downloads ~3.5 GB GGUF on first run.

Gemma 4 live tests now short-circuit unless the host matches the compiled backend:
- `metal`: macOS on Apple Silicon, Metal-capable GPU, and Apple's Metal toolchain installed.
- `cuda` / `cudnn`: NVIDIA GPU detected on the host.
- no GPU backend feature: tests print a skip reason and return immediately.
