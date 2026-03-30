# Data Model: Adapter — Mistral

**Feature**: 018-adapter-mistral
**Date**: 2026-03-30

## Entities

### MistralStreamFn

Primary adapter type implementing `StreamFn`.

| Field | Type | Description |
|---|---|---|
| `base` | `AdapterBase` | HTTP client, base URL, API key |

**Relationships**: Implements `StreamFn` trait (core). Uses `OaiConverter` (openai_compat) for message serialization. Uses `parse_oai_sse_stream` for SSE parsing with post-processing.

**Constructor**: `new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self`

### MistralIdMap

Bidirectional tool call ID mapping for a single stream invocation.

| Field | Type | Description |
|---|---|---|
| `harness_to_mistral` | `HashMap<String, String>` | Maps harness IDs (`call_*`) → Mistral IDs (9-char) |
| `mistral_to_harness` | `HashMap<String, String>` | Maps Mistral IDs → harness IDs |

**Lifecycle**: Created at start of `stream()`, populated during message conversion, consumed during response parsing. Dropped when stream completes.

### Request Normalization (outbound)

Transformations applied to `OaiChatRequest` before sending:

| Field | OpenAI Value | Mistral Value | Action |
|---|---|---|---|
| `max_tokens` | from `max_completion_tokens` | `max_tokens` | Rename field |
| `stream_options` | `{"include_usage": true}` | omitted | Remove field |
| `seed` | `seed` | `random_seed` | Rename field (if present) |
| Tool call IDs in messages | `call_*` format | 9-char `[a-zA-Z0-9]` | Remap via `MistralIdMap` |
| Message ordering | `[..., tool_result, user, ...]` | Insert synthetic assistant | Fix ordering constraint |

### Response Normalization (inbound)

Transformations applied to `AssistantMessageEvent` stream after parsing:

| Event | Condition | Action |
|---|---|---|
| `ToolCallStart` | ID is 9-char Mistral format | Remap to harness ID via `MistralIdMap` |
| `Done` | `finish_reason == "model_length"` | Map to `StopReason::MaxTokens` (same as `length`) |
| `Done` | `finish_reason == "error"` | Convert to error event |

## State Transitions

```
MistralStreamFn::stream() called
  → Build MistralIdMap from context tool results
  → Convert messages via OaiConverter (with ID remapping + ordering fix)
  → Construct OaiChatRequest (with field normalization)
  → POST to {base_url}/v1/chat/completions
  → Parse SSE via parse_oai_sse_stream
  → Post-process events (ID remapping, finish_reason normalization)
  → Yield AssistantMessageEvent values
  → Drop MistralIdMap
```

## Validation Rules

- Tool call IDs generated for Mistral MUST match `^[a-zA-Z0-9]{9}$`
- `max_tokens` MUST NOT exceed model's context window minus prompt tokens
- `stream_options` MUST NOT be present in the request body
- Message sequence MUST NOT have `tool` role immediately followed by `user` role
