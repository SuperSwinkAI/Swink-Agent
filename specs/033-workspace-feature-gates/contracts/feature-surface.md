# Feature Surface Contract: Workspace Feature Gates

**Date**: 2026-03-25 (**Updated 2026-09-14**: re-verified every row against `adapters/Cargo.toml`, `local-llm/Cargo.toml`, and root `Cargo.toml` — issue #1313)

This document defines the public feature flag contract that consumers depend on. Changes to feature names or semantics are breaking changes. The feature-to-feature implications below are enforced by `adapters/tests/suite/cargo_manifest.rs`; update both together.

## swink-agent-adapters

### Features

| Feature | Implies | Description |
|---------|---------|-------------|
| `default` | — | No adapters enabled by default |
| `all` | `anthropic`, `openai`, `ollama`, `gemini`, `proxy`, `azure`, `bedrock`, `mistral`, `xai`, `responses`, `codex` | Enables all 11 provider features |
| `full` | `all` | Alias for `all` |
| `anthropic` | — | Anthropic Messages API |
| `openai` | `openai-compat`, `responses` | OpenAI Chat Completions API (plus the Responses shell) |
| `responses` | — | Generic OpenAI Responses-API adapter |
| `codex` | `responses` (+ `dep:swink-agent-auth`, `dep:base64`) | ChatGPT-subscription Codex provider over the Responses shell. Opt-in; read the `codex` module docs before enabling |
| `ollama` | — | Ollama local inference (NDJSON) |
| `gemini` | — | Google Gemini API |
| `proxy` | — | Generic proxy endpoint |
| `azure` | `dep:swink-agent-auth` | Azure OpenAI (OpenAI-compatible) |
| `bedrock` | AWS SigV4 / smithy deps | AWS Bedrock Converse API |
| `mistral` | — | Mistral (OpenAI-compatible) |
| `xai` | `openai-compat` | xAI Grok (OpenAI-compatible). Does **not** imply `openai` |
| `openai-compat` | — | Internal umbrella implied by `openai` and `xai`; enables no adapter on its own |
| `__no_default_features_sentinel` | — | Hidden feature-leak detection flag; not consumer-facing |

### Public Re-exports by Feature

```
anthropic → AnthropicStreamFn
openai    → OpenAiStreamFn, OpenAiWire, InvalidOpenAiWire, OPENAI_API_ENV
responses → ResponsesStreamFn
codex     → CodexStreamFn, CodexError, CODEX_BASE_URL, CODEX_CLIENT_ID,
            CODEX_REDIRECT_URI, CODEX_DEFAULT_CREDENTIAL_KEY, DEFAULT_ORIGINATOR,
            codex_authorization_config
ollama    → OllamaStreamFn
gemini    → GeminiStreamFn
proxy     → ProxyStreamFn
azure     → AzureStreamFn, AzureAuth, AzureCloud
bedrock   → BedrockStreamFn
mistral   → MistralStreamFn
xai       → XAiStreamFn
```

### Always Available (no feature required)

```
pub mod classify;
pub mod sse;
pub mod convert;
pub use remote_presets::*;   // incl. is_provider_compiled
pub use base::ensure_default_crypto_provider;
pub use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
```

## swink-agent-local-llm

### Features

| Feature | Implies | Description |
|---------|---------|-------------|
| `gemma4` | — | Gemma 4 model presets (`ModelPreset::Gemma4E2B`, `Gemma4E4B`, `Gemma4_26B`, `Gemma4_31B`) and channel-thought parsing |
| `metal` | `llama-cpp-2/metal` | Apple Metal GPU acceleration |
| `cuda` | `llama-cpp-2/cuda` | NVIDIA CUDA GPU acceleration |
| `cudnn` | `cuda` | Alias for `cuda` (no separate llama-cpp-2 flag) |
| `vulkan` | `llama-cpp-2/vulkan` | Vulkan GPU acceleration |

No default features. Without any backend feature, CPU inference is used.

### Public API

Unchanged. All types always available when the crate is compiled:
```
LocalStreamFn, LocalModel, ModelConfig, ModelState,
ModelPreset, LocalModelError, EmbeddingModel, EmbeddingConfig,
ProgressCallbackFn, ProgressEvent, LocalPresetError,
DEFAULT_LOCAL_PRESET_ID, default_local_connection
```

`gemma4` adds the Gemma 4 `ModelPreset` variants (plus gated helpers such as `is_gemma4`); it adds or removes no top-level types.

## swink-agent (root)

### Features

| Feature | Activates | Description |
|---------|-----------|-------------|
| `default` | `builtin-tools`, `transfer` | Current behavior preserved |
| `builtin-tools` | `dep:sha2` | BashTool, ReadFileTool, WriteFileTool, EditFileTool, `builtin_tools()` |
| `transfer` | — | TransferToAgentTool, TransferChain, TransferSignal, TransferError |
| `testkit` | — | Test utility re-exports (mock StreamFn, tools, builders) |
| `plugins` | — | Plugin trait, PluginRegistry, NamespacedTool |
| `artifact-store` | `dep:bytes` | Artifact storage traits and types |
| `artifact-tools` | `artifact-store` | ListArtifactsTool, LoadArtifactTool, SaveArtifactTool, `artifact_tools()` |
| `hot-reload` | `dep:notify` | File-watcher-based hot reload |
| `tiktoken` | `dep:tiktoken-rs` | Precise token counting via tiktoken |
| `otel` | tracing-opentelemetry stack | OpenTelemetry tracing export |

> **Note:** The root crate does not forward adapter or local-llm features. Consumers depend on `swink-agent-adapters` and `swink-agent-local-llm` directly for provider selection.

## swink-agent-tui

### Features

| Feature | Activates | Description |
|---------|-----------|-------------|
| `default` | `cli`, `builtin-tools`, `transfer` | Standalone `swink` binary behavior |
| `cli` | `adapters` | `swink` binary with remote adapters |
| `builtin-tools` | `swink-agent/builtin-tools` | Root built-in local tools |
| `transfer` | `swink-agent/transfer` | Root TransferToAgent tool |
| `full` | `local`, `cli`, `builtin-tools`, `transfer` | Everything |

The TUI depends on `swink-agent` with `default-features = false`; root local-execution features are only enabled through the forwarding features above.

## swink-agent-tui-remote

Depends on `swink-agent` and `swink-agent-tui` with `default-features = false`, so neither `swink-agent/builtin-tools` nor `swink-agent/transfer` is activated. Guarded by `tui-remote/tests/feature_surface.rs`.

## Consumer Examples

```toml
# Minimal: just the agent loop
swink-agent = { path = "../Swink-Agent", default-features = false }

# Core + specific adapters (depend on sub-crate directly)
swink-agent = { path = "../Swink-Agent" }
swink-agent-adapters = { path = "../Swink-Agent/adapters", default-features = false, features = ["anthropic", "openai"] }

# All adapters
swink-agent-adapters = { path = "../Swink-Agent/adapters", features = ["all"] }
```
