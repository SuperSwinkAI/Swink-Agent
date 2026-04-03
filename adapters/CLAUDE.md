# CLAUDE.md — LLM Provider Adapters

## Scope

`adapters/` — StreamFn implementations for 9 LLM providers (7 implemented, 2 stubs). Separate crate to keep provider-specific deps out of core.

## Feature Gates

Each adapter is feature-gated. Defaults are minimal, and `full` enables the current batteries-included set.

```toml
# Selective compilation
swink-agent-adapters = { features = ["anthropic", "openai"] }

# Everything
swink-agent-adapters = { features = ["full"] }

# Minimal default build
swink-agent-adapters = {}
```

| Feature | Module | Extra deps | Status |
|---------|--------|-----------|--------|
| `anthropic` | `anthropic.rs` | — | Implemented |
| `openai` | `openai.rs` | — | Implemented |
| `ollama` | `ollama.rs` | — | Implemented |
| `gemini` | `google.rs` | — | Implemented |
| `proxy` | `proxy.rs` | — | Implemented |
| `azure` | `azure.rs` | — | Implemented |
| `bedrock` | `bedrock.rs` | `sha2`, `hmac`, `chrono`, `aws-smithy-eventstream`, `aws-smithy-types` | Implemented |
| `mistral` | `mistral.rs` | — | Implemented |
| `xai` | `xai.rs` | — | Implemented |

**Always compiled** (shared infra): `base`, `sse`, `classify`, `convert`, `finalize`, `openai_compat`, `remote_presets`.

**Note**: `gemini` feature gates `mod google` — the feature name matches the public type (`GeminiStreamFn`) and user mental model, not the file name.

## Key Facts

- All adapters implement `StreamFn` (Send + Sync). Provider modules are private; public API is re-exports only.
- `MessageConverter` trait (defined in core, re-exported from `swink_agent::convert`) eliminates per-adapter boilerplate — except Anthropic, which has its own `convert_messages` (system prompt is top-level, thinking blocks filtered).
- `ProxyStreamFn` moved here from core. Import: `swink_agent_adapters::ProxyStreamFn`.
- The `classify` and `sse` modules are public but documented as internal utilities with no stability contract. External StreamFn implementors should depend only on `swink_agent`.
- SSE-backed adapters should reuse `adapters/src/sse.rs` helpers when possible instead of carrying their own byte-to-line parser. `ProxyStreamFn` now follows that shared path too.
- `openai_compat` is shared by `openai`, `azure`, `mistral`, `xai` — compiles unconditionally but has `allow(dead_code)` when none of its consumers are enabled.
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
- **Auth** — Anthropic: `x-api-key`. OpenAI: `Authorization: Bearer`. Ollama: none. Mistral: `Authorization: Bearer`. Azure: `api-key` header (API key) or `Authorization: Bearer` (Entra ID OAuth2).
- **Azure adapter** — reuses `openai_compat` for SSE parsing and tool calls. Azure-specific: URL construction (`{base_url}/chat/completions`), dual auth (`AzureAuth::ApiKey` / `AzureAuth::EntraId`), content filter detection (`finish_reason: "content_filter"` in stream, `ContentFilterBlocked` in HTTP error body → `ContentFiltered` error). Entra ID tokens cached with 5-min proactive refresh margin.
- **Mistral API divergences from OpenAI** — Tool call IDs must be exactly 9-char `[a-zA-Z0-9]` (Mistral rejects OpenAI-style `call_*` IDs with HTTP 422). `stream_options` field rejected (usage comes automatically in final chunk). Must use `max_tokens` not `max_completion_tokens`. `finish_reason: "model_length"` is Mistral-specific (mapped to `Length` in shared parser). User messages cannot immediately follow tool messages (synthetic assistant message inserted). Adapter holds `AdapterBase` directly (Azure pattern) with custom `MistralChatRequest` and `MistralIdMap` for bidirectional ID remapping.

## Live Tests

```bash
cargo test -p swink-agent-adapters -- --ignored          # all
cargo test -p swink-agent-adapters --test anthropic_live -- --ignored
cargo test -p swink-agent-adapters --test openai_live -- --ignored
cargo test -p swink-agent-adapters --test mistral_live -- --ignored
cargo test -p swink-agent-adapters --test azure_live -- --ignored
```

Live tests are `#[ignore]`, use cheap models, 30s timeout, validate event sequences not text content. `.env` loaded via dotenvy.
