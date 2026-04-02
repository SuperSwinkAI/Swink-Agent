# Research: Adapter xAI

**Feature**: 017-adapter-xai | **Date**: 2026-04-02

## R1: xAI API Protocol Compatibility

**Decision**: xAI follows the OpenAI chat completions protocol — reuse `OpenAiStreamFn` internals directly.

**Rationale**: xAI explicitly documents OpenAI SDK compatibility (`base_url="https://api.x.ai/v1"`). The SSE streaming format is identical: `chat.completion.chunk` objects, `choices[].delta.content`, `data: [DONE]` sentinel. Message roles, tool call format, and tool definitions all match OpenAI.

**Alternatives considered**:
- Custom xAI-specific protocol implementation → rejected (unnecessary, xAI uses OpenAI protocol)
- Proactive normalizer layer (Mistral pattern) → rejected (no known xAI-specific divergences unlike Mistral's ID format issues)

## R2: Authentication

**Decision**: Bearer token via `Authorization: Bearer <XAI_API_KEY>` header.

**Rationale**: Matches xAI docs and all other OpenAI-compatible adapters (OpenAI, Mistral, Azure).

**Alternatives considered**: None — Bearer is the only documented auth mechanism.

## R3: stream_options Behavior

**Decision**: Send `stream_options: { include_usage: true }` in request body (same as OpenAI adapter). If xAI rejects it, lenient parsing handles the error gracefully.

**Rationale**: xAI may include usage data automatically in streaming chunks (some reports suggest usage appears in every chunk, not just the final one). The OpenAI-compatible shared parser already handles usage extraction from any chunk. Sending `stream_options` is harmless if ignored and beneficial if honored. If xAI rejects the field entirely, the HTTP error path handles it cleanly and we can add a custom request type (like Mistral did) as a follow-up.

**Alternatives considered**:
- Custom `XAiChatRequest` without `stream_options` (Mistral pattern) → deferred unless testing reveals rejection
- Not requesting usage at all → rejected (budget policies need token counts)

## R4: Model Catalog — Current Grok Models

**Decision**: Update catalog from stale grok-3 entries to current grok-4.x lineup.

**Rationale**: xAI's model page (as of April 2026) lists:

| Model ID | Context | Cost (in/out per 1M) | Capabilities |
|---|---|---|---|
| `grok-4.20-0309-reasoning` | 2M | $2.00 / $6.00 | text, tools, vision, structured output, reasoning |
| `grok-4.20-0309-non-reasoning` | 2M | $2.00 / $6.00 | text, tools, vision, structured output |
| `grok-4-1-fast-reasoning` | 2M | $0.20 / $0.50 | text, tools, vision, structured output, reasoning |
| `grok-4-1-fast-non-reasoning` | 2M | $0.20 / $0.50 | text, tools, vision, structured output |
| `grok-4.20-multi-agent-0309` | 2M | $2.00 / $6.00 | text, tools, vision, structured output, reasoning |

Cached input pricing: $0.20/M (grok-4.20), $0.05/M (grok-4-1-fast). All models support tool calling and vision.

**Alternatives considered**:
- Keep grok-3 entries → rejected (models no longer listed, likely deprecated)
- Minimal catalog (1-2 models) → rejected (clarification decided comprehensive coverage)

## R5: Existing Implementation State

**Decision**: The adapter code (`adapters/src/xai.rs`) already exists as a thin wrapper delegating to `OpenAiStreamFn`. This is architecturally correct. The remaining work is:

1. Update model catalog presets (stale grok-3 → current grok-4.x)
2. Add live tests (text streaming, tool calls, error handling)
3. Verify `stream_options` behavior with real xAI API

**Rationale**: The existing 50-line wrapper is the correct architecture — xAI's protocol is identical to OpenAI's with no known quirks requiring normalization. The feature gate, `lib.rs` re-export, and `Cargo.toml` entry are already in place.

## R6: Known xAI Quirks

**Decision**: No dedicated quirk handling needed at this time.

**Findings**:
- Usage may appear in every streaming chunk (not just final) — shared parser already handles this
- No tool call ID format restrictions (unlike Mistral's 9-char limit)
- `finish_reason` values match OpenAI (`stop`, `tool_calls`, `length`)
- Reasoning models (`*-reasoning`) may return reasoning content — not yet handled by the adapter but not blocking (content arrives as normal text deltas)
- `presencePenalty`, `frequencyPenalty`, `stop` params unsupported by reasoning models — not currently passed by the adapter

**Alternatives considered**: Proactive normalizer → rejected per clarification (lenient parsing only).
