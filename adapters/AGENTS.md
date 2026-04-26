# AGENTS.md — LLM Provider Adapters

## Scope

`adapters/` — `StreamFn` implementations for 9 LLM providers. Separate crate to keep provider deps out of core.

## Feature Gates

`default = []`, `full = ["all"]`. Individual flags: `anthropic`, `openai`, `ollama`, `gemini`, `proxy`, `azure`, `bedrock`, `mistral`, `xai`.

- `gemini` gates `mod google`. `proxy` activates `eventsource-stream`. `bedrock` activates `sha2`/`hmac`/`chrono`/`aws-smithy-*`.
- Provider-only support crates stay optional under owning feature (e.g. `swink-agent-auth` under `azure`).
- Always compiled (shared infra): `base`, `sse`, `classify`, `convert`, `finalize`, `openai_compat`, `remote_presets`.
- `openai_compat` shared by `openai`, `azure`, `mistral`, `xai`.
- Don't run `--all-features` on generic Linux — `metal`/`accelerate` are Apple-only.
- Keep feature-leak sentinels behind hidden `__no_default_features_sentinel` cargo feature.

## Key Invariants

- All adapters implement `StreamFn` (Send + Sync). Provider modules private; public API is re-exports only.
- `MessageConverter` trait eliminates per-adapter boilerplate — except Anthropic (system prompt top-level, thinking filtered).
- Pre-stream failures must emit `[Start, Error]`, not bare `Error`.
- Malformed JSON for known event types is a non-retryable protocol fault → `error(...)`. Only transport failures → `error_network(...)`.
- Transport EOF without terminal frame → drain open blocks, `error_network(...)`.
- Cancellation must race initial HTTP send via `tokio::select!`, not just pre-check token.
- Raw payload observers must thread `StreamOptions.on_raw_payload` through callback-aware parsers.

## Provider Notes

| Adapter | Protocol | Sentinel |
|---|---|---|
| Anthropic | SSE | `event: message_stop` |
| OpenAI | SSE | `data: [DONE]` |
| Ollama | NDJSON | `done: true` |
| Mistral | SSE | `data: [DONE]` |
| Azure | SSE | `data: [DONE]` |
| Proxy | SSE | `type: done`/`type: error` |

- **Anthropic** — thinking budget = `thinking_level` + `thinking_budgets`, capped `max_tokens - 1`. SSE state machine remaps block indices after filtering.
- **OpenAI** — multi-provider: works with any `/v1/chat/completions`-compatible endpoint.
- **Ollama** — NDJSON parser must buffer raw bytes until full newline-delimited record (UTF-8 safety).
- **OAI compat** — buffer tool-call args until non-empty `function.name` known; never synthesize empty-name `ToolCallStart`.
- **Mistral** — 9-char alphanumeric tool IDs, no `stream_options`, `max_tokens` not `max_completion_tokens`, `finish_reason: "model_length"`.
- **Azure** — race Entra token acquisition with cancellation. Classify 4xx auth as non-retryable `error_auth`.
- **Bedrock** — `send_converse_stream()` must race cancellation in init state. EOF is never a successful terminal.
- **Google** — `function_call.args` chunks are full snapshots (overwrite, not delta). Terminal errors must flush buffered tool-call state.
- **Proxy** — `data: [DONE]` without prior typed `done`/`error` event is a protocol error.
- **Auth** — Anthropic `x-api-key`, OpenAI/Mistral `Bearer`, Ollama none, Azure `api-key` or `Bearer` (Entra OAuth2 via `SingleFlightTokenSource`).

## Live Tests

```bash
cargo test -p swink-agent-adapters -- --ignored
cargo test -p swink-agent-adapters --test <provider>_live -- --ignored
```

`#[ignore]`, cheap models, 30s timeout, validate event sequences not content.
