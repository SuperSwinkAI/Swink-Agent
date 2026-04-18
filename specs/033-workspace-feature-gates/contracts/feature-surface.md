# Feature Surface Contract: Workspace Feature Gates

**Date**: 2026-03-25

This document defines the public feature flag contract that consumers depend on. Changes to feature names or semantics are breaking changes.

## swink-agent-adapters

### Features

| Feature | Status | Description |
|---------|--------|-------------|
| `default` | — | No adapters enabled by default |
| `full` / `all` | — | Enables all 9 adapter features |
| `anthropic` | Implemented | Anthropic Messages API |
| `openai` | Implemented | OpenAI Chat Completions API |
| `ollama` | Implemented | Ollama local inference (NDJSON) |
| `gemini` | Implemented | Google Gemini API |
| `proxy` | Implemented | Generic proxy endpoint |
| `azure` | Implemented | Azure OpenAI (OpenAI-compatible) |
| `bedrock` | Implemented | AWS Bedrock Converse API |
| `mistral` | Implemented | Mistral (OpenAI-compatible) |
| `xai` | Implemented | xAI Grok (OpenAI-compatible, implies `openai`) |

### Public Re-exports by Feature

```
anthropic → AnthropicStreamFn
openai    → OpenAiStreamFn
ollama    → OllamaStreamFn
gemini    → GeminiStreamFn
proxy     → ProxyStreamFn
azure     → AzureStreamFn
bedrock   → BedrockStreamFn
mistral   → MistralStreamFn
xai       → XAiStreamFn
```

### Always Available (no feature required)

```
pub mod classify;
pub mod sse;
pub mod convert;
pub fn remote_presets::*;
```

## swink-agent-local-llm

### Features

| Feature | Forwards to | Description |
|---------|------------|-------------|
| `metal` | `llama-cpp-2/metal` | Apple Metal GPU acceleration |
| `cuda` | `llama-cpp-2/cuda` | NVIDIA CUDA GPU acceleration |
| `vulkan` | `llama-cpp-2/vulkan` | Vulkan GPU acceleration |

No default backend feature. Without any backend feature, CPU inference is used.

### Public API

Unchanged. All types always available when the crate is compiled:
```
LocalStreamFn, LocalModel, ModelConfig, ModelState,
ModelPreset, LocalModelError, EmbeddingModel, EmbeddingConfig,
ProgressCallbackFn, ProgressEvent
```

## swink-agent (root)

### Features

| Feature | Activates | Description |
|---------|-----------|-------------|
| `default` | `builtin-tools`, `transfer` | Current behavior preserved |
| `builtin-tools` | — | BashTool, ReadFileTool, WriteFileTool |
| `transfer` | — | TransferToAgent tool |
| `testkit` | — | Test utility re-exports (mock StreamFn, tools, builders) |
| `plugins` | — | Plugin trait, PluginRegistry, NamespacedTool |
| `artifact-store` | `dep:bytes` | Artifact storage traits and types |
| `artifact-tools` | `artifact-store` | Artifact read/write agent tools |
| `hot-reload` | `dep:notify` | File-watcher-based hot reload |
| `tiktoken` | `dep:tiktoken-rs` | Precise token counting via tiktoken |
| `otel` | tracing-opentelemetry stack | OpenTelemetry tracing export |

> **Note:** The root crate does not forward adapter or local-llm features. Consumers depend on `swink-agent-adapters` and `swink-agent-local-llm` directly for provider selection.

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
