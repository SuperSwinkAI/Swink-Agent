# Research: Adapter — Mistral

**Feature**: 018-adapter-mistral
**Date**: 2026-03-30

## R1: Mistral API Protocol Compatibility

**Decision**: Mistral uses OpenAI chat completions protocol with significant divergences requiring a request normalizer (outbound) and response normalizer (inbound).

**Rationale**: While the SSE streaming format and message structure are largely OpenAI-compatible, multiple parameter and format differences cause 422 rejection errors if sent verbatim. A normalizer layer is required on both request and response sides.

**Alternatives considered**:
- Pure delegation (rejected — Mistral rejects `stream_options`, `max_completion_tokens`, OpenAI-style tool_call_ids)
- Fork openai_compat (rejected — divergences are adapter-specific, not protocol-level)

## R2: Known Mistral API Divergences from OpenAI

### Request-Side Divergences (outbound normalizer)

| OpenAI Parameter | Mistral Equivalent | Impact |
|---|---|---|
| `max_completion_tokens` | `max_tokens` | Mistral returns 422 "Extra inputs are not permitted" |
| `stream_options: {"include_usage": true}` | Not supported | Mistral returns 422; usage included automatically in final chunk |
| `seed` | `random_seed` | Different parameter name |
| Tool call IDs: `call_PTLP8xhu3uwZk4l3nlnrrJha` | 9-char alphanumeric `[a-zA-Z0-9]{9}` | Mistral returns 422 if ID doesn't match format |

### Response-Side Divergences (inbound normalizer)

| Divergence | Detail | Normalizer Action |
|---|---|---|
| `finish_reason: "model_length"` | Mistral-specific, means model's own length limit | Map to `length` |
| `finish_reason: "error"` | Mistral-specific generation error | Map to error event |
| Tool calls may arrive as complete objects | Not incremental deltas like OpenAI | Handle full tool call in single chunk |
| `ThinkChunk` content type | Reasoning model traces | Skip or map to thinking events |

### Message Ordering Constraint

Mistral rejects `user` messages immediately following `tool` messages. The message converter must insert a synthetic `assistant` message (content: `""` or `"OK"`) between tool result and user message sequences.

## R3: Authentication and Endpoint

**Decision**: `Authorization: Bearer <key>` to `https://api.mistral.ai/v1/chat/completions`.

**Rationale**: Confirmed via Mistral docs. Standard Bearer auth, standard chat completions path. Default base URL is `https://api.mistral.ai`.

## R4: Complete Model Catalog

**Decision**: Include all currently available chat/agent models. Exclude embeddings, moderation, and OCR (not chat completions).

### Frontier / Generalist Models

| Preset ID | Model ID | Context | Max Output | Capabilities |
|---|---|---|---|---|
| `mistral_large` | `mistral-large-latest` | 256,000 | 8,192 | text, tools, images_in, streaming, structured_output |
| `mistral_medium` | `mistral-medium-latest` | 128,000 | 8,192 | text, tools, images_in, streaming |
| `mistral_small` | `mistral-small-latest` | 256,000 | 8,192 | text, tools, images_in, streaming, structured_output |
| `ministral_3b` | `ministral-3b-2512` | 256,000 | 8,192 | text, images_in, streaming |
| `ministral_8b` | `ministral-8b-2512` | 256,000 | 8,192 | text, images_in, streaming |
| `ministral_14b` | `ministral-14b-2512` | 256,000 | 8,192 | text, images_in, streaming |

### Reasoning Models

| Preset ID | Model ID | Context | Max Output | Capabilities |
|---|---|---|---|---|
| `magistral_medium` | `magistral-medium-2509` | 40,000 | 8,192 | text, tools, streaming |
| `magistral_small` | `magistral-small-2509` | 40,000 | 8,192 | text, tools, streaming |

### Code-Specialized Models

| Preset ID | Model ID | Context | Max Output | Capabilities |
|---|---|---|---|---|
| `codestral` | `codestral-latest` | 256,000 | 8,192 | text, tools, streaming |
| `devstral` | `devstral-2512` | 256,000 | 8,192 | text, tools, streaming |

### Vision / Multimodal Models

| Preset ID | Model ID | Context | Max Output | Capabilities |
|---|---|---|---|---|
| `pixtral_large` | `pixtral-large-2411` | 128,000 | 8,192 | text, tools, images_in, streaming |
| `pixtral_12b` | `pixtral-12b-2409` | 128,000 | 8,192 | text, images_in, streaming |

## R5: Normalizer Architecture

**Decision**: `MistralStreamFn` holds `AdapterBase` directly (like Azure) instead of wrapping `OpenAiStreamFn`. This allows the adapter to customize both request construction and response parsing.

**Rationale**: The divergences are too numerous for a simple event-stream wrapper:
1. Request body needs `max_tokens` instead of `max_completion_tokens`, no `stream_options`
2. Tool call IDs in outbound messages need remapping to 9-char format
3. Message ordering needs synthetic assistant message insertion
4. Response stream needs `model_length` → `length` mapping

The adapter reuses `openai_compat` types (`OaiChatRequest`, `OaiMessage`, `OaiConverter`, `parse_oai_sse_stream`) but constructs the request and post-processes the response itself.

**Alternatives considered**:
- Wrap `OpenAiStreamFn` with stream map (rejected — can't intercept request construction)
- Add "quirks" config to `openai_compat` (rejected — pollutes shared layer with provider-specific concerns)

## R6: Tool Call ID Remapping

**Decision**: Generate Mistral-compatible 9-char alphanumeric IDs and maintain a bidirectional mapping for the duration of a stream call.

**Rationale**: The harness uses OpenAI-style `call_*` IDs internally. When sending tool results back to Mistral, the adapter must translate IDs. The mapping is scoped to a single `stream()` invocation.

**Format**: `[a-zA-Z0-9]{9}` — generated via `rand` (already a workspace dependency).

## R7: Test Strategy

**Decision**: Full test parity with OpenAI adapter plus Mistral-specific divergence tests.

**Tests required**:
1. Text streaming (happy path)
2. Tool call streaming (single and multi-tool)
3. Error classification (401, 429, 500, timeout)
4. Stream cancellation
5. Usage tracking (from final chunk, no stream_options)
6. `model_length` finish reason mapping
7. Tool call ID format (9-char alphanumeric)
8. Message ordering (tool result → user message insertion)
9. Live integration test (`#[ignore]`, `MISTRAL_API_KEY`)

## Sources

- Mistral API docs: https://docs.mistral.ai/api
- Mistral function calling: https://docs.mistral.ai/capabilities/function_calling
- Mistral models: https://docs.mistral.ai/getting-started/models
- Tool call ID format: https://github.com/sst/opencode/issues/1680
- `max_completion_tokens` rejection: https://github.com/openclaw/openclaw/issues/47079
- `stream_options` rejection: https://github.com/openai/openai-agents-python/issues/442
- Pydantic-AI Mistral adapter: https://github.com/pydantic/pydantic-ai
