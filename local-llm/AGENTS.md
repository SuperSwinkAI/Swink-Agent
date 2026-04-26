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
- **Non-Gemma `<think>` parsing is stateful across chunks** — `local-llm/src/stream.rs` now tracks split `<think>` / `</think>` delimiters across token boundaries and flushes any buffered partial delimiter fragments on terminal paths so streamed thinking/text content is not dropped at chunk boundaries.
- **Context capped at 8192 tokens** (NoPE architecture). Override via `LOCAL_CONTEXT_LENGTH` env var.
- **llama-cpp-2 version pin** — API is actively evolving; pin to specific minor version.
- **SmolLM3 is fully supported** — `llama-cpp-2` (llama.cpp) natively supports the SmolLM3 GGUF architecture. No fail-fast rejection needed.
- **`with_progress` returns `Result`** — call before cloning the `Arc`.
- **`ModelState` split** — public `ModelState` (re-exported from lib.rs) vs internal `InternalModelState` (holds the llama.cpp model handle). Stream code uses `InternalModelState`.
- **`LlamaContext` is `!Send`** — llama.cpp contexts cannot be sent across threads. Inference uses a dedicated thread pattern (`std::thread::spawn` + channel) to keep the context on a single OS thread.
- **Embedding method naming** — `embed(text)` for single text, `embed_batch(texts)` for batch. Errors use `LocalModelError::Embedding` variant.
- **Gemma 4 works on CPU** — unlike the previous mistralrs-based implementation which hung silently on CPU, llama.cpp handles Gemma 4 GGUF inference on CPU correctly (though slowly). GPU acceleration (`--features gemma4,metal` on Apple Silicon, `--features gemma4,cuda` on NVIDIA) is strongly recommended for usable performance.
- **`cudnn` is a local-llm feature alias for `cuda`** — tests and docs may gate on either name, but both compile the same llama.cpp CUDA backend in this crate.
- **Gemma 4 live tests are runtime-gated** — `local-llm/tests/common/mod.rs` uses `swink_agent::testing::should_run_test()` to detect OS/GPU support before calling `ensure_ready()`. This avoids hanging on unsupported hosts and keeps the skip reason close to the real constraint.
- **Metal builds need Apple's Metal toolchain** — on macOS, compiling with `--features metal` requires the `metal` compiler from Apple's Metal Toolchain. If the build script says `cannot execute tool 'metal'`, install it with `xcodebuild -downloadComponent MetalToolchain`.
- **Gemma 4 uses GGUF** — all models (including Gemma 4) now use GGUF format loaded via `LlamaModel::load_from_file`. Gemma 4 GGUF repos are from `bartowski/` (e.g., `bartowski/google_gemma-4-E2B-it-GGUF`).
- **Gemma 4 model family detection** — `ModelConfig::is_gemma4()` checks `repo_id` for `"gemma-4"` or `"gemma4"` substrings (behind `gemma4` feature flag).
- **Gemma 4 thinking mode** — `<|think|>\n` prepended to system prompt in `convert.rs` when `config.is_gemma4() && thinking_enabled`. llama.cpp has no `think: true` API for direct inference.
- **Gemma 4 thinking output** — `ChannelThoughtParser` in `stream.rs` handles cross-chunk `<|channel>thought\n...<channel|>` delimiters. Stateful 4-state machine (Normal → PartialOpen → InThinking → PartialClose).
- **Gemma 4 tool calls** — `ToolCallParser` in `stream.rs` handles cross-chunk `<|tool_call>call:{name}{args}<tool_call|>` format. IDs are generated as UUIDs. `Gemma4LocalConverter` wraps tool results as `<|tool_result>{tool_call_id}\n{text}<tool_result|>`.
- **Gemma 4 terminal finalization must drain parser buffers** — normal done, error, cancellation, and EOF paths must flush `ChannelThoughtParser` / `ToolCallParser` state. For `FinishReason::Length`, incomplete native tool calls should remain tool-call events with partial JSON so core max-token recovery can synthesize the error tool result.
- **Default local tool calls** — non-Gemma streaming parses plain `call:{name}{json_object}` output into standard `ToolCall*` events. Keep this parser balanced-JSON and chunk-safe; incomplete calls must flush back as text on terminal paths rather than disappearing.
- **Gemma 4 partial delimiter matching must be UTF-8 safe** — when scanning for split `<|channel>thought\n` and `<tool_call|>` delimiters, only slice `&str` values at character boundaries. Reuse the shared suffix helper in `stream.rs`; raw byte-offset suffix slicing can panic on multibyte output.
- **Local stream finish reasons must survive finalization** — `runner.rs` must propagate whether generation ended naturally or because it exhausted `max_tokens`, and `stream.rs` must preserve `Length` ahead of synthesized `ToolUse` so the core loop can recover incomplete tool-call JSON.
- **Per-request stream overrides belong in `GenerateOptions`, not `RunnerConfig`** — the loaded `LlamaRunner` is shared across requests, so `StreamOptions.max_tokens` / `temperature` must be translated into per-inference generation options at call time instead of mutating model-level defaults.
- **`TokenEvent::Error` is a terminal-finalization path** — `stream.rs` must drain open text/thinking/tool-call blocks before emitting the terminal `Error`, matching the cancellation/EOF paths and the core stream contract.
- **Cancellation terminals are semantic, not generic** — `stream.rs` must emit `StopReason::Aborted` for both pre-start cancellation and in-stream cancellation finalization. A plain `AssistantMessageEvent::error(...)` masks intentional aborts as runtime failures to the core loop.
- **Pre-cancel must win before model readiness** — `local_stream()` must check `CancellationToken` before calling `ensure_ready()`, otherwise an already-aborted run can still kick off heavyweight local model download/load work before reporting the abort.
- **In-flight readiness must also race cancellation** — a one-time pre-check is insufficient because `ensure_ready().await` can block behind download/load work after the stream has already started waiting. Keep readiness behind a `tokio::select!` against the same `CancellationToken`.
- **`hf-hub` progress handlers are clone-per-chunk** — download byte progress must aggregate through shared state; per-clone counters misreport resumed bytes and parallel chunk updates.
- **Loading progress needs runner-level substeps** — `loader.rs` only knows the coarse Loading phase. To surface meaningful `LoadingProgress` updates, `model.rs` / `embedding.rs` must pass the shared callback into `LlamaRunner::load_with_progress()`, which emits backend-init and GGUF-load messages from inside the blocking runner setup.
- **Two converter types** — `LocalConverter` (SmolLM3) and `Gemma4LocalConverter` (Gemma 4) both implement `MessageConverter`. `convert_context_messages` dispatches based on `config.is_gemma4()`.
- **Assistant tool-call context must not flatten to text-only** — local conversion should preserve assistant `ToolCall` blocks with call id, tool name, and arguments so later tool results remain meaningful in the prompt history.
- **LazyLoader waiters must treat `Unloaded` as terminal for the current attempt** — `wait_until_ready()` now returns on `Unloaded`/`Failed` as well as `Ready`, and `ensure_ready()` re-checks loader state after every wakeup so an `unload()` during another caller's load does not strand waiters forever.
- Workspace-wide `cargo build` / `test` / `clippy` now compile `llama-cpp-sys-2` through this crate, so contributors need LLVM/libclang installed and Windows contributors commonly need `LIBCLANG_PATH` pointed at the LLVM `bin` directory.

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
