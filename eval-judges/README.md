# swink-agent-eval-judges

[![Crates.io](https://img.shields.io/crates/v/swink-agent-eval-judges.svg)](https://crates.io/crates/swink-agent-eval-judges)
[![Docs.rs](https://docs.rs/swink-agent-eval-judges/badge.svg)](https://docs.rs/swink-agent-eval-judges)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](https://github.com/SuperSwinkAI/Swink-Agent/blob/main/LICENSE-MIT)

Provider-backed `JudgeClient` implementations for [`swink-agent-eval`](https://crates.io/crates/swink-agent-eval).

This crate owns the HTTP-facing judge adapters that spec 043 layers on top of
the core judge traits in `swink-agent-eval`: provider clients, shared retry and
cooperative cancellation, plus the bounded batching wrapper used by
judge-backed evaluators.

## Features

The crate is fully opt-in. Enable only the providers you ship.

| Feature | Exposes | Endpoint shape | Auth |
| --- | --- | --- | --- |
| `anthropic` | `AnthropicJudgeClient`, `BlockingAnthropicJudgeClient` | `POST /v1/messages` | `x-api-key` header |
| `openai` | `OpenAiJudgeClient`, `OpenAIJudgeClient`, `BlockingOpenAiJudgeClient`, `BlockingOpenAIJudgeClient` | `POST /v1/chat/completions` | Bearer token |
| `bedrock` | `BedrockJudgeClient`, `BlockingBedrockJudgeClient` | `POST /model/<model>/invoke` | Bearer token / pre-signed upstream credential |
| `gemini` | `GeminiJudgeClient`, `BlockingGeminiJudgeClient` | `POST /v1beta/models/<model>:generateContent?key=...` | API key query parameter |
| `mistral` | `MistralJudgeClient`, `BlockingMistralJudgeClient` | `POST /v1/chat/completions` | Bearer token |
| `azure` | `AzureJudgeClient`, `BlockingAzureJudgeClient` | `POST /chat/completions?api-version=...` on a deployment URL | `api-key` header |
| `xai` | `XaiJudgeClient`, `BlockingXaiJudgeClient` | `POST /v1/chat/completions` | Bearer token |
| `ollama` | `OllamaJudgeClient`, `BlockingOllamaJudgeClient` | `POST /api/chat` | None |
| `proxy` | `ProxyJudgeClient`, `BlockingProxyJudgeClient` | `POST` to your configured judge URL | Bearer token |
| `all-judges` | All provider features above | Mixed | Mixed |
| `live-judges` | Marker for live/provider integration coverage | N/A | N/A |

Every provider client uses the same shared retry builder from
`eval-judges/src/client.rs`:

- default retry policy: 6 total attempts, exponential backoff, 4 minute max delay, jitter on
- cancellation: every attempt races the supplied `CancellationToken`
- batching: `BatchedJudgeClient` enforces `batch_size` in `1..=128` and falls back to sequential dispatch for providers without native batch APIs

## Installation

```toml
[dependencies]
swink-agent-eval = { version = "0.9", features = ["judge-core"] }
swink-agent-eval-judges = { version = "0.9", features = ["openai"] }
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

## Quick Start

```rust,ignore
use std::sync::Arc;

use swink_agent_eval::judge::JudgeRegistry;
use swink_agent_eval_judges::OpenAIJudgeClient;

let client = OpenAIJudgeClient::new(
    "https://api.openai.com",
    std::env::var("OPENAI_API_KEY")?,
    "gpt-4o-mini",
);

let judges = JudgeRegistry::builder(Arc::new(client), "gpt-4o-mini").build()?;
```

## Provider Notes

### Anthropic

- Base URL example: `https://api.anthropic.com`
- Constructor: `AnthropicJudgeClient::new(base_url, api_key, model)`
- Optional tuning: `with_anthropic_version`, `with_max_tokens`, `with_retry_policy`, `with_cancellation`

### OpenAI-compatible family

These providers all speak an OpenAI-style chat-completions wire shape and
return the verdict JSON inside the assistant message content:

- OpenAI: `OpenAiJudgeClient::new("https://api.openai.com", api_key, model)`
- Mistral: `MistralJudgeClient::new("https://api.mistral.ai", api_key, model)`
- xAI: `XaiJudgeClient::new("https://api.x.ai", api_key, model)`

Each supports `with_temperature`, `with_retry_policy`, and
`with_cancellation`.

### Azure OpenAI

- Base URL must already include the deployment path, for example:
  `https://example.openai.azure.com/openai/deployments/gpt-4o`
- Constructor: `AzureJudgeClient::new(base_url, api_key, model)`
- Optional tuning: `with_api_version`, `with_temperature`, `with_retry_policy`, `with_cancellation`

### Bedrock

- Base URL example: `https://bedrock-runtime.us-east-1.amazonaws.com`
- Constructor: `BedrockJudgeClient::new(base_url, api_key, model)`
- Uses the Anthropic-on-Bedrock request shape
- Optional tuning: `with_anthropic_version`, `with_max_tokens`, `with_retry_policy`, `with_cancellation`

### Gemini

- Base URL example: `https://generativelanguage.googleapis.com`
- Constructor: `GeminiJudgeClient::new(base_url, api_key, model)`
- Authentication is carried in the request query string
- Optional tuning: `with_temperature`, `with_retry_policy`, `with_cancellation`

### Ollama

- Base URL example: `http://localhost:11434`
- Constructor: `OllamaJudgeClient::new(base_url, model)`
- No API key is required
- Optional tuning: `with_temperature`, `with_retry_policy`, `with_cancellation`

### Proxy

- Constructor: `ProxyJudgeClient::new(base_url, api_key)`
- `base_url` should be the full judge endpoint, for example:
  `https://proxy.example.com/judge`
- The proxy owns backend model selection; the client sends only the rendered prompt

## Testing

Provider tests are wiremock-backed and do not make live LLM calls in the
default test matrix. Run one provider at a time or validate the full crate
surface:

```bash
cargo test -p swink-agent-eval-judges --features openai --test openai_test
cargo test -p swink-agent-eval-judges --features anthropic --test anthropic_test
cargo build -p swink-agent-eval-judges --features all-judges
```

---

Part of the [Swink-Agent](https://github.com/SuperSwinkAI/Swink-Agent)
workspace.
