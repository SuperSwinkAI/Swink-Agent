# CLAUDE.md — Local Inference

## Scope

`local-llm/` — On-device LLM inference via mistral.rs. Two models: SmolLM3-3B (text/tools) and EmbeddingGemma-300M (embeddings).

## References

- **PRD:** §7 (Streaming Interface), §14.1 (Dependencies)
- **Architecture:** `docs/architecture/streaming/README.md`

## Key Facts

- `LocalModel` and `EmbeddingModel` use `Arc<Inner>` pattern for cheap cloning and concurrent access — same pattern as `Agent` in core.
- Both models are lazily downloaded from HuggingFace on first use via `ensure_ready()`. Downloads cached in `~/.cache/huggingface/hub/`.
- `ModelState` lifecycle: `Unloaded → Downloading → Loading → Ready { runner }` (or `Failed { error }`). State transitions serialized by `RwLock`.
- `Notify` pattern for `wait_until_ready()` — same as `agent.rs::idle_notify`.
- `forbid(unsafe_code)` at crate root. If mistralrs macros ever conflict, downgrade to `deny(unsafe_code)` and document here.
- `convert` module is now private (`mod` not `pub mod`) — internal implementation detail.
- Error types use `#[derive(thiserror::Error)]` — convenience constructors (`download()`, `loading()`, `inference()`) are preserved.

## Lessons Learned

- **SmolLM3 `<think>` tags** — SmolLM3 uses `<think>...</think>` tags for chain-of-thought reasoning. The stream adapter parses these boundaries and routes content to `ThinkingStart/ThinkingDelta/ThinkingEnd` events. Tag detection is simple string matching (not regex) for speed.
- **NoPE (No Positional Encoding) architecture** — SmolLM3 uses NoPE which may affect long-context performance. Context length is capped at 8192 tokens by default to save memory. Increase via `LOCAL_CONTEXT_LENGTH` env var if needed.
- **Non-streaming wrapper** — The `LocalStreamFn` uses `send_chat_request` (non-streaming) and wraps the response into the event protocol. This simplifies the implementation while still producing valid `AssistantMessageEvent` sequences. Future optimization: switch to `stream_chat_request` for true token-by-token streaming.
- **Cost is always zero** — Local inference has no per-token cost. `Cost` fields are all 0.0.
- **mistralrs version pin** — Pin to a specific minor version to avoid breaking API changes. The mistralrs API is actively evolving and not yet stable.
- **Embedding model uses `EmbeddingModelBuilder`** — mistral.rs has a dedicated builder for embedding models, separate from `GgufModelBuilder`/`TextModelBuilder`. The `send_embedding_request` method returns vectors directly.
- **`with_progress` returns `Result`** — `LocalModel::with_progress()` and `EmbeddingModel::with_progress()` return `Result<Self, LocalModelError>` instead of panicking on shared `Arc`. Call before cloning.
- **Use `AssistantMessageEvent::error()`** — local stream uses the core constructor instead of a local `error_event` function.

## Live Tests

Live tests download models (~2.1 GB combined) and are `#[ignore]`'d:

```bash
# Run text generation tests
cargo test -p swink-agent-local-llm --test local_live -- --ignored

# Run embedding tests
cargo test -p swink-agent-local-llm --test embedding_live -- --ignored
```
