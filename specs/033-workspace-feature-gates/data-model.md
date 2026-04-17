# Data Model: 033 Workspace Feature Gates

**Date**: 2026-03-25

This feature has no runtime data model. All entities are compile-time Cargo feature flags that control module visibility. The "data model" here is the feature dependency graph.

## Feature Flag Topology

### swink-agent-adapters

```
default = ["all"]
all = [anthropic, openai, ollama, gemini, proxy, azure, bedrock, mistral, xai]

anthropic   → (marker only, no extra deps)
openai      → (marker only)
ollama      → (marker only)
gemini      → (marker only)
proxy       → dep:eventsource-stream
azure       → (marker only, uses openai_compat which is always compiled)
bedrock     → dep:sha2
mistral     → (marker only, delegates to openai)
xai         → (marker only, delegates to openai)
```

**Always compiled**: base, sse, classify, convert, finalize, openai_compat, remote_presets

### swink-agent-local-llm

```
(no default backend — CPU inference when no backend feature enabled)

metal       → llama-cpp-2/metal
cuda        → llama-cpp-2/cuda
vulkan      → llama-cpp-2/vulkan
```

### swink-agent-tui (existing, unchanged)

```
default = ["local"]
local = [dep:swink-agent-local-llm]
```

### swink-agent (root)

```
default = ["builtin-tools"]

# Existing
builtin-tools = []
test-helpers  = []

# Adapter forwarding (each activates optional dep on adapters crate)
anthropic    → dep:swink-agent-adapters, swink-agent-adapters/anthropic
openai       → dep:swink-agent-adapters, swink-agent-adapters/openai
ollama       → dep:swink-agent-adapters, swink-agent-adapters/ollama
gemini       → dep:swink-agent-adapters, swink-agent-adapters/gemini
proxy        → dep:swink-agent-adapters, swink-agent-adapters/proxy
azure        → dep:swink-agent-adapters, swink-agent-adapters/azure
bedrock      → dep:swink-agent-adapters, swink-agent-adapters/bedrock
mistral      → dep:swink-agent-adapters, swink-agent-adapters/mistral
xai          → dep:swink-agent-adapters, swink-agent-adapters/xai
adapters-all → dep:swink-agent-adapters, swink-agent-adapters/all

# Local LLM forwarding
local-llm        → dep:swink-agent-local-llm
local-llm-metal  → dep:swink-agent-local-llm, swink-agent-local-llm/metal
local-llm-cuda   → dep:swink-agent-local-llm, swink-agent-local-llm/cuda

# TUI
tui → dep:swink-agent-tui
```

## Module Visibility Matrix (adapters crate)

| Module | Type | Compiled when |
|--------|------|--------------|
| base | Shared | Always |
| sse | Shared | Always |
| classify | Shared | Always |
| convert | Shared | Always |
| finalize | Shared | Always |
| openai_compat | Shared | Always |
| remote_presets | Shared | Always |
| anthropic | Provider | `feature = "anthropic"` |
| openai | Provider | `feature = "openai"` |
| ollama | Provider | `feature = "ollama"` |
| google | Provider | `feature = "gemini"` |
| proxy | Provider | `feature = "proxy"` |
| azure | Provider | `feature = "azure"` |
| bedrock | Provider | `feature = "bedrock"` |
| mistral | Provider | `feature = "mistral"` |
| xai | Provider | `feature = "xai"` |
