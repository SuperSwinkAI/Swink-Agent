# CLAUDE.md — LLM Provider Adapters

## Scope

`adapters/` — StreamFn implementations for Ollama, Anthropic, and OpenAI. Separate crate to keep provider-specific dependencies out of the core.

## References

- **PRD:** §7 (Streaming Interface), §14.1 (Adapters Dependencies), §15.1 (Adapters Crate)
- **Architecture:** `docs/architecture/streaming/README.md`

## Key Facts

- All adapters implement `StreamFn` (Send + Sync, object-safe).
- `MessageConverter` trait (convert.rs) eliminates per-adapter message format boilerplate. Each adapter implements 4 methods; `convert_messages()` handles iteration.
- Tests use wiremock to mock provider responses — see `adapters/tests/`.
- Provider modules (`anthropic`, `ollama`, `openai`) are now private (`mod` not `pub mod`). Public API is re-exports only (`AnthropicStreamFn`, `OllamaStreamFn`, `OpenAiStreamFn`).
- `error_event()` in `convert.rs` now delegates to `AssistantMessageEvent::error()` from core.

## Protocols

| Adapter | Protocol | Endpoint | Sentinel |
|---|---|---|---|
| AnthropicStreamFn | SSE (event+data lines) | `/v1/messages` | `event: message_stop` |
| OpenAiStreamFn | SSE (data: prefix) | `/v1/chat/completions` | `data: [DONE]` |
| OllamaStreamFn | NDJSON (one object per line) | `/api/chat` | `done: true` in object |

## Lessons Learned

- **Anthropic thinking blocks require budget math** — thinking budget is computed from `model.thinking_level` + `thinking_budgets` map, capped to `max_tokens - 1`. Thinking blocks are stripped from outgoing requests (`convert_messages` skips them) because the API doesn't accept them back.
- **OpenAI adapter is multi-provider** — works with OpenAI, vLLM, LM Studio, Groq, Together, and any `/v1/chat/completions`-compatible endpoint. Single implementation, no subclassing.
- **SSE state machine in Anthropic** — uses `SseStreamState` to track block indices (provider index to harness index mapping). Content blocks arrive with provider-assigned indices that don't match harness indices due to thinking block filtering.
- **Ollama has no sentinel line** — unlike SSE adapters, the NDJSON stream ends when the `done` field is `true` in a response object. Parser checks this per-line.
- **Bearer token auth** — Anthropic uses `x-api-key` header. OpenAI uses `Authorization: Bearer`. Ollama has no auth by default.
- **convert.rs is private** — `MessageConverter` is `pub(crate)`, not re-exported. It's an internal abstraction, not part of the public API.
- **Anthropic `convert_messages` is intentionally separate** — does NOT use the shared `MessageConverter` trait because Anthropic's API requires system prompt as a top-level field and thinking blocks must be filtered.

## Live Tests

Live tests hit real provider APIs and are isolated from normal `cargo test` runs via `#[ignore]`.

**Running:**
```bash
# All live tests (reads keys from .env via dotenvy)
cargo test -p swink-agent-adapters -- --ignored

# Just Anthropic
cargo test -p swink-agent-adapters --test anthropic_live -- --ignored

# Just OpenAI
cargo test -p swink-agent-adapters --test openai_live -- --ignored
```

**Conventions:**
- Live test files are separate from mock tests: `anthropic_live.rs`, `openai_live.rs`.
- Every live test is `#[ignore]` + `#[tokio::test]`.
- Each test wraps its stream in `tokio::time::timeout(30s)` to prevent hangs.
- Use cheap models (`claude-haiku-4-5-20251001`, `gpt-4o-mini`) and short prompts to minimize cost.
- `dotenvy` (dev-dependency) loads `.env` automatically — no manual `export` needed.
- Tests validate stream event sequences, not exact text content (LLM output is non-deterministic).
