# Data Model: Gemma 4 Local Default (Direct Inference)

**Feature**: 041-gemma4-local-default  
**Date**: 2026-04-04

## Entities

### ModelPreset (extended)

Existing enum in `local-llm/src/preset.rs`. Gains three new variants.

| Variant | repo_id | filename | context_length | gpu_layers | Builder |
|---------|---------|----------|---------------|------------|---------|
| `SmolLM3_3B` (existing) | `bartowski/SmolLM3-3B-GGUF` | `SmolLM3-3B-Q4_K_M.gguf` | 8192 | 0 | GgufModelBuilder |
| `EmbeddingGemma300M` (existing) | `google/gemma-embedding-300m` | (default) | 2048 | 0 | EmbeddingModelBuilder |
| **`Gemma4_E2B`** (new, default) | `bartowski/google_gemma-4-E2B-it-GGUF` | `gemma-4-E2B-it-Q4_K_M.gguf` | 131072 | 0 | MultimodalModelBuilder |
| **`Gemma4_E4B`** (new) | `bartowski/google_gemma-4-E4B-it-GGUF` | `gemma-4-E4B-it-Q4_K_M.gguf` | 131072 | 0 | MultimodalModelBuilder |
| **`Gemma4_26B`** (new) | `bartowski/google_gemma-4-26B-it-GGUF` | `gemma-4-26B-it-Q4_K_M.gguf` | 262144 | 0 | MultimodalModelBuilder |

All new variants are behind `#[cfg(feature = "gemma4")]`.

Environment variable overrides (existing mechanism, shared across all presets):
- `LOCAL_MODEL_REPO` → overrides `repo_id`
- `LOCAL_MODEL_FILE` → overrides `filename`
- `LOCAL_CONTEXT_LENGTH` → overrides `context_length`
- `LOCAL_GPU_LAYERS` → overrides `gpu_layers`

### ModelConfig (extended)

Existing struct in `local-llm/src/model.rs`. No new fields required.

New method:

| Method | Returns | Description |
|--------|---------|-------------|
| `is_gemma4(&self)` | `bool` | `true` if `repo_id` contains `"gemma-4"` or `"gemma4"`. Used for model-family branching in builder selection, thinking token injection, and output parsing. |

### ChannelThoughtParser (new)

Stateful parser for Gemma 4's `<|channel>thought\n...<channel|>` delimiter format. Replaces `extract_thinking_delta()` for Gemma 4 models.

| Field | Type | Description |
|-------|------|-------------|
| `state` | `ParserState` | Current state: `Normal`, `InThinking`, `PartialOpen`, `PartialClose` |
| `buffer` | `String` | Accumulated characters during partial delimiter matching |

| State | Transitions |
|-------|------------|
| `Normal` | → `PartialOpen` (on `<` that could begin `<\|channel>thought\n`) → `Normal` (on non-delimiter text) |
| `PartialOpen` | → `InThinking` (on complete `<\|channel>thought\n`) → `Normal` (on non-matching continuation, flush buffer as text) |
| `InThinking` | → `PartialClose` (on `<` that could begin `<channel\|>`) → `InThinking` (on thinking content) |
| `PartialClose` | → `Normal` (on complete `<channel\|>`) → `InThinking` (on non-matching continuation, flush buffer as thinking) |

Output per chunk: `(Option<ThinkingContent>, Option<TextContent>)` — zero or one of each per call.

### ToolCallParser (new, P3 priority)

Stateful parser for Gemma 4's native tool call format. Deferred until core inference is stable.

| Delimiter | Format |
|-----------|--------|
| Tool call open | `<\|tool_call>` |
| Tool call body | `call:{function_name}{json_arguments}` |
| Tool call close | `<tool_call\|>` |
| Tool result open | `<\|tool_result>` |
| Tool result body | `{function_name}\n{result_text}` |
| Tool result close | `<tool_result\|>` |

## State Transitions

### ModelState (unchanged)

The existing lifecycle state machine remains unchanged for Gemma 4 models:

```
Unloaded → Downloading → Loading → Ready
                                  → Failed
```

The only difference is the builder used during `Loading` (MultimodalModelBuilder vs GgufModelBuilder). The `LoaderState::Ready { runner: mistralrs::Model }` type is the same for both builders.

### Thinking Parser State Machine

```
Normal ──[see opening delimiter prefix]──→ PartialOpen
PartialOpen ──[complete match]──→ InThinking
PartialOpen ──[mismatch]──→ Normal (flush as text)
InThinking ──[see closing delimiter prefix]──→ PartialClose
PartialClose ──[complete match]──→ Normal
PartialClose ──[mismatch]──→ InThinking (flush as thinking)
```

## Relationships

```
ModelPreset ──[config()]──→ ModelConfig ──[is_gemma4()]──→ builder selection
                                        ──[is_gemma4()]──→ parser selection
                                        ──[is_gemma4()]──→ thinking token injection

ModelConfig ──[ChatBackend::build()]──→ GgufModelBuilder (non-Gemma4)
                                     ──→ MultimodalModelBuilder (Gemma4)

LocalStreamFn ──[stream()]──→ StreamState ──[process_content_delta()]──→ extract_thinking_delta() (non-Gemma4)
                                          ──[process_content_delta()]──→ ChannelThoughtParser (Gemma4)
```

## Feature Gate Boundary

| Component | Behind `gemma4` flag | Always compiled |
|-----------|---------------------|-----------------|
| `ModelPreset::Gemma4_E2B/E4B/26B` | Yes | |
| `ModelConfig::is_gemma4()` | Yes | |
| `MultimodalModelBuilder` code path | Yes | |
| `ChannelThoughtParser` | Yes | |
| `<\|think\|>` injection in convert.rs | Yes | |
| `ToolCallParser` | Yes | |
| `DEFAULT_LOCAL_PRESET_ID` change | Yes (conditional) | SmolLM3-3B when disabled |
| `GgufModelBuilder` code path | | Yes |
| `extract_thinking_delta()` | | Yes |
| `LazyLoader`, `LoaderBackend`, `LocalStreamFn` | | Yes |
| `ModelConfig` struct | | Yes |
| `SmolLM3_3B`, `EmbeddingGemma300M` presets | | Yes |
