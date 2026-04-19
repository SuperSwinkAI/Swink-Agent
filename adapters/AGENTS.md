# AGENTS.md — LLM Provider Adapters

## Scope

`adapters/` — `StreamFn` implementations for 9 LLM providers. Separate crate to keep provider-specific deps out of core.

## Feature Gates

`default = []`, `full = ["all"]`. Individual flags: `anthropic`, `openai`, `ollama`, `gemini`, `proxy`, `azure`, `bedrock`, `mistral`, `xai`.

- `gemini` gates `mod google` — feature name matches public type `GeminiStreamFn`, not the file name.
- `proxy` activates `eventsource-stream` dep. `bedrock` activates `sha2`/`hmac`/`chrono`/`aws-smithy-*` deps.
- Provider-only support crates must stay optional and hang off the owning feature. In particular, `swink-agent-auth` belongs under `azure = ["dep:swink-agent-auth"]`; leaving it unconditional leaks Azure auth into non-Azure builds.
- **Always compiled** (shared infra): `base`, `sse`, `classify`, `convert`, `finalize`, `openai_compat`, `remote_presets`.
- `openai_compat` is shared by `openai`, `azure`, `mistral`, `xai` — compiles unconditionally but has `allow(dead_code)` when none enabled.
- Portable CI must not run `--all-features` on a generic Linux runner — `metal`/`accelerate` pull Apple-only deps. Exclude `swink-agent-local-llm` or target explicit feature sets.
- Keep feature-leak sentinels behind an explicit hidden cargo feature (`__no_default_features_sentinel`). Workspace test runs can unify adapter features from other packages, making an always-on sentinel a false failure.

## Key Facts

- All adapters implement `StreamFn` (Send + Sync). Provider modules are private; public API is re-exports only.
- `MessageConverter` trait (from `swink_agent::convert`) eliminates per-adapter boilerplate — except Anthropic, which has its own `convert_messages` (system prompt is top-level, thinking blocks filtered).
- `ProxyStreamFn` moved here from core. Import: `swink_agent_adapters::ProxyStreamFn`.
- SSE-backed adapters should reuse `adapters/src/sse.rs` helpers; `ProxyStreamFn` follows that shared path.
- `remote_presets` module feature-gates preset key sub-modules and `build_remote_connection` match arms per provider.

## Protocols

| Adapter | Protocol | Endpoint | Sentinel |
|---|---|---|---|
| Anthropic | SSE | `/v1/messages` | `event: message_stop` |
| OpenAI | SSE | `/v1/chat/completions` | `data: [DONE]` |
| Ollama | NDJSON | `/api/chat` | `done: true` in object |
| Mistral | SSE | `/v1/chat/completions` | `data: [DONE]` |
| Azure | SSE | `{base_url}/chat/completions` | `data: [DONE]` |
| Proxy | SSE | `{base_url}/v1/stream` | `type: done`/`type: error` |

## Lessons Learned

- **Anthropic thinking blocks** — budget = `thinking_level` + `thinking_budgets` map, capped to `max_tokens - 1`. Stripped from outgoing requests (API rejects them). SSE state machine (`SseStreamState`) remaps block indices because provider indices don't match harness indices after filtering.
- **OpenAI adapter is multi-provider** — works with any `/v1/chat/completions`-compatible endpoint (vLLM, LM Studio, Groq, Together, etc.).
- **Auth** — Anthropic: `x-api-key`. OpenAI/Mistral: `Authorization: Bearer`. Ollama: none. Azure: `api-key` header (API key) or `Authorization: Bearer` (Entra ID OAuth2, cached with 5-min proactive refresh margin; use `swink-agent-auth::SingleFlightTokenSource` — an adapter-local `RwLock<Option<_>>` cache does not deduplicate concurrent refreshes).
- **Bedrock** — malformed JSON for known event types (`messageStart`, `contentBlock*`, `messageStop`, `metadata`) must terminate the stream after draining open blocks. Only truly unknown event types are safe to skip.
- **Bedrock** — transport EOF is never a successful terminal signal by itself. Only explicit Bedrock terminal frames (`metadata`, with `messageStop` carrying the stop reason when present) may produce `Done`; bare EOF must drain open blocks and emit `error_network(...)`.
- **Mistral divergences from OpenAI** — Tool call IDs must be exactly 9-char `[a-zA-Z0-9]`. `stream_options` field rejected (usage comes in final chunk). Must use `max_tokens` not `max_completion_tokens`. `finish_reason: "model_length"` is Mistral-specific. User messages cannot immediately follow tool messages (synthetic assistant message inserted).
- Any failure before the provider yields its first streaming payload must still emit `[Start, Error]`. Returning only a terminal `Error` makes `accumulate_message()` fail with `no Start event found`.
- `finish_reason == "content_filter"` must be routed through `OaiSseStreamState.terminal_error`; emitting an inline error and then consuming a later `[DONE]` produces a duplicate terminal event that `accumulate_message` rejects.
- In `src/google.rs`, `function_call.args` chunks are full snapshots, not deltas. Always overwrite the buffered args, including `null`, or a later empty snapshot can leave stale JSON from an earlier chunk.
- In `src/google.rs`, terminal error exits must flush buffered tool-call snapshots and finalize open blocks before emitting `Error`; otherwise `accumulate_message()` can observe dangling block state on parse/transport failures.
- In `src/ollama.rs`, the NDJSON parser must buffer raw bytes until it has a full newline-delimited record. Decoding each transport chunk independently with `from_utf8_lossy` corrupts split multibyte UTF-8.
- In `src/ollama.rs` and `src/proxy.rs`, deterministic parse/protocol faults (malformed NDJSON/SSE JSON, proxy terminal-frame violations, unexpected `done` EOF) must use `AssistantMessageEvent::error(...)`, not `error_network(...)`; only transport failures stay retryable.
- In `src/openai_compat.rs`, buffer tool-call arguments and delay `ToolCallStart` until a non-empty `function.name` is known; some providers stream arguments before the name.
- Runtime SSE adapters must thread `StreamOptions.on_raw_payload` into the callback-aware shared parser (`sse_data_lines_with_callback`). The callback-free helper silently disables payload observers.
- In `src/proxy.rs`, treat transport `data: [DONE]` as a protocol error unless the proxy has already emitted a typed `done` or `error` JSON event.
- In `src/oai_transport.rs`, gate `OaiAdapterShell` helper methods to the provider features that actually call them. Azure-only builds reuse the request/parse pipeline but not the Bearer-auth convenience helpers, and otherwise `cargo clippy --no-default-features --features azure -D warnings` trips dead-code failures.

## Live Tests

```bash
cargo test -p swink-agent-adapters -- --ignored
cargo test -p swink-agent-adapters --test anthropic_live -- --ignored
cargo test -p swink-agent-adapters --test openai_live -- --ignored
cargo test -p swink-agent-adapters --test mistral_live -- --ignored
cargo test -p swink-agent-adapters --test azure_live -- --ignored
```

Live tests are `#[ignore]`, use cheap models, 30s timeout, validate event sequences not text content. `.env` loaded via dotenvy.
