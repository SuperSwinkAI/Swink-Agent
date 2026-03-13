# CLAUDE.md — LLM Provider Adapters

## Scope

`adapters/` — StreamFn implementations for Ollama, Anthropic, OpenAI, and Proxy. Separate crate to keep provider-specific deps out of core.

## Key Facts

- All adapters implement `StreamFn` (Send + Sync). Provider modules are private; public API is re-exports only.
- `MessageConverter` trait (convert.rs, `pub(crate)`) eliminates per-adapter boilerplate — except Anthropic, which has its own `convert_messages` (system prompt is top-level, thinking blocks filtered).
- `ProxyStreamFn` moved here from core. Import: `swink_agent_adapters::ProxyStreamFn`.

## Protocols

| Adapter | Protocol | Endpoint | Sentinel |
|---|---|---|---|
| Anthropic | SSE | `/v1/messages` | `event: message_stop` |
| OpenAI | SSE | `/v1/chat/completions` | `data: [DONE]` |
| Ollama | NDJSON | `/api/chat` | `done: true` in object |
| Proxy | SSE | `{base_url}/v1/stream` | `type: done`/`type: error` |

## Lessons Learned

- **Anthropic thinking blocks** — budget = `thinking_level` + `thinking_budgets` map, capped to `max_tokens - 1`. Stripped from outgoing requests (API rejects them). SSE state machine (`SseStreamState`) remaps block indices because provider indices don't match harness indices after filtering.
- **OpenAI adapter is multi-provider** — works with any `/v1/chat/completions`-compatible endpoint (vLLM, LM Studio, Groq, Together, etc.).
- **Auth** — Anthropic: `x-api-key`. OpenAI: `Authorization: Bearer`. Ollama: none.

## Live Tests

```bash
cargo test -p swink-agent-adapters -- --ignored          # all
cargo test -p swink-agent-adapters --test anthropic_live -- --ignored
cargo test -p swink-agent-adapters --test openai_live -- --ignored
```

Live tests are `#[ignore]`, use cheap models, 30s timeout, validate event sequences not text content. `.env` loaded via dotenvy.
