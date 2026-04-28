# AGENTS.md — Local Inference

## Scope

`local-llm/` — On-device inference via llama.cpp (`llama-cpp-2`). SmolLM3-3B (default), Gemma 4 E2B (opt-in `gemma4` feature), EmbeddingGemma-300M (embeddings). GGUF format.

## Key Facts

- Models lazily downloaded from HuggingFace on first `ensure_ready()`. Cached in `~/.cache/huggingface/hub/`.
- `ModelState` lifecycle: `Unloaded → Downloading → Loading → Ready | Failed`.
- `LlamaContext` is `!Send` — inference uses dedicated thread + channel pattern.
- Per-request overrides (`max_tokens`, `temperature`) go in `GenerateOptions`, not `RunnerConfig`.
- Two converters: `LocalConverter` (SmolLM3) and `Gemma4LocalConverter`. Both preserve assistant `ToolCall` blocks in context.

## Key Invariants

- **Terminal finalization** — all exit paths (done, error, cancel, EOF) must drain parser buffers. `FinishReason::Length` preserves incomplete tool calls for core recovery. `TokenEvent::Error` drains open blocks before emitting.
- **Cancellation** — check token before `ensure_ready()`, race readiness via `tokio::select!`, emit `StopReason::Aborted`.
- **SmolLM3 `<think>` parsing** is stateful across chunks — flushes partial delimiter fragments on terminal paths.
- **Gemma 4 thinking** — `ChannelThoughtParser` handles `<|channel>thought\n...<channel|>` (4-state machine).
- **Gemma 4 tool calls** — `ToolCallParser` handles `<|tool_call>call:{name}{args}<tool_call|>`. IDs are UUIDs.
- **Partial delimiter matching must be UTF-8 safe** — only slice at character boundaries.
- **`LazyLoader` waiters** — `wait_until_ready()` returns on `Unloaded`/`Failed`/`Ready`; `ensure_ready()` re-checks after every wakeup.
- **`hf-hub` progress handlers are clone-per-chunk** — aggregate through shared state.

## Build & Test

```bash
cargo build -p swink-agent-local-llm
cargo build -p swink-agent-local-llm --features gemma4
```

Live tests: `cargo test -p swink-agent-local-llm --test local_live -- --ignored` (SmolLM3 ~1.9GB, Gemma4 ~3.5GB first run). Gemma 4 tests auto-skip on unsupported hosts. Metal builds need Apple's Metal toolchain.

Intentional duplication between `model.rs` and `embedding.rs`: both follow `Arc<Inner>` + state-machine pattern but have different runner types and public APIs. Only two implementations — not worth a generic.
