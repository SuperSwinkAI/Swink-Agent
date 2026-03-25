# Feature Surface Contract: Workspace Feature Gates

**Date**: 2026-03-25

This document defines the public feature flag contract that consumers depend on. Changes to feature names or semantics are breaking changes.

## swink-agent-adapters

### Features

| Feature | Status | Description |
|---------|--------|-------------|
| `default` | — | Enables `all` |
| `all` | — | Enables all 9 adapter features |
| `anthropic` | Implemented | Anthropic Messages API |
| `openai` | Implemented | OpenAI Chat Completions API |
| `ollama` | Implemented | Ollama local inference (NDJSON) |
| `gemini` | Implemented | Google Gemini API |
| `proxy` | Implemented | Generic proxy endpoint |
| `azure` | Stub | Azure OpenAI (OpenAI-compatible) |
| `bedrock` | Stub | AWS Bedrock Converse API |
| `mistral` | Stub | Mistral (OpenAI-compatible) |
| `xai` | Stub | xAI Grok (OpenAI-compatible) |

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
| `metal` | `mistralrs/metal` | Apple Metal GPU acceleration |
| `cuda` | `mistralrs/cuda` | NVIDIA CUDA GPU acceleration |
| `cudnn` | `mistralrs/cudnn` | NVIDIA cuDNN acceleration |
| `flash-attn` | `mistralrs/flash-attn` | Flash Attention (implies cuda) |
| `mkl` | `mistralrs/mkl` | Intel MKL math acceleration |
| `accelerate` | `mistralrs/accelerate` | Apple Accelerate framework |

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
| `default` | `builtin-tools` | Current behavior preserved |
| `builtin-tools` | — | BashTool, ReadFileTool, WriteFileTool |
| `test-helpers` | — | Test utility re-exports |
| `anthropic` | adapters crate + feature | Anthropic adapter |
| `openai` | adapters crate + feature | OpenAI adapter |
| `ollama` | adapters crate + feature | Ollama adapter |
| `gemini` | adapters crate + feature | Gemini adapter |
| `proxy` | adapters crate + feature | Proxy adapter |
| `azure` | adapters crate + feature | Azure adapter |
| `bedrock` | adapters crate + feature | Bedrock adapter |
| `mistral` | adapters crate + feature | Mistral adapter |
| `xai` | adapters crate + feature | xAI adapter |
| `adapters-all` | adapters crate + all | All adapters |
| `local-llm` | local-llm crate | Local inference (CPU) |
| `local-llm-metal` | local-llm crate + metal | Local + Metal |
| `local-llm-cuda` | local-llm crate + cuda | Local + CUDA |
| `tui` | TUI crate | Terminal UI |

## Consumer Examples

```toml
# Minimal: just the agent loop
swink-agent = { path = "../Swink-Agent", default-features = false }

# Anthropic + OpenAI only
swink-agent = { path = "../Swink-Agent", features = ["anthropic", "openai"] }

# Everything
swink-agent = { path = "../Swink-Agent", features = ["adapters-all", "local-llm-metal", "tui"] }

# SuperSwink-Core typical usage
swink-agent = { path = "../Swink-Agent", features = ["anthropic", "openai", "ollama", "gemini"] }
```
