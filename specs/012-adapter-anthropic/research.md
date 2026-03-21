# Research: Adapter: Anthropic

**Feature**: 012-adapter-anthropic | **Date**: 2026-03-20

## Decision 1: SSE Protocol Handling

**Question**: Should the Anthropic adapter use the shared `SseStreamParser` or its own SSE parsing?

**Decision**: Custom SSE parsing via `sse_event_lines()` that pairs `event:` and `data:` lines into `SseLine::Event { event_type, data }` tuples. The Anthropic SSE format requires event-type labels (e.g., `content_block_start`, `content_block_delta`, `message_stop`) to drive the state machine, so the shared `sse_data_lines()` combinator (which filters to `Data`/`Done` only) is insufficient.

**Rationale**: Anthropic's SSE protocol uses `event:` labels to distinguish between `message_start`, `content_block_start`, `content_block_delta`, `content_block_stop`, `message_delta`, `message_stop`, and `error`. The event type drives state transitions in `SseStreamState`. The shared `sse_data_lines()` strips event types, which would lose this critical dispatch information. A custom line parser (~60 lines) pairs `event:` with the subsequent `data:` line before yielding.

**Alternatives rejected**:
- *Shared `sse_data_lines()` combinator*: Strips event-type labels; Anthropic needs them for state machine dispatch.
- *`eventsource-stream` crate*: Adds a dependency for functionality that is straightforward to implement and already exists in the codebase.

## Decision 2: Thinking Block Support and Budget Control

**Question**: How should thinking configuration be resolved and how should thinking blocks be handled in the stream?

**Decision**: `resolve_thinking()` reads `ThinkingLevel` and optional `thinking_budgets` map from `ModelSpec`. Default budgets are hardcoded per level (Minimal=1024, Low=2048, Medium=5000, High=10000, ExtraHigh=20000). Budget is capped to `max_tokens - 1` (Anthropic requires strict less-than). When thinking is enabled, temperature is forced to `None` (Anthropic requires default temperature=1). Thinking blocks stream as `ThinkingStart`/`ThinkingDelta`/`ThinkingEnd` events with an optional signature on end.

**Rationale**: The provider enforces the budget server-side; the adapter only needs to include it in the request. Capping to `max_tokens - 1` silently prevents API rejection — this is a normal edge case (callers set budgets by level, not absolute token count). Temperature suppression is an Anthropic API requirement when thinking is enabled.

**Alternatives rejected**:
- *Separate thinking configuration struct on `AnthropicStreamFn`*: Over-couples; `ModelSpec` already carries thinking level and budgets.
- *Error on budget exceeding max_tokens*: Overly strict; silent capping matches the "level-based" mental model callers use.

## Decision 3: Message Conversion (Bespoke, Not Shared Trait)

**Question**: Should the Anthropic adapter use the shared `MessageConverter` trait?

**Decision**: Bespoke `convert_messages()` function that returns `(Option<String>, Vec<AnthropicMessage>)` — system prompt as a separate field, messages as a vector. Thinking blocks and empty text blocks are stripped from outgoing assistant messages. Consecutive tool results are merged into a single `user` message.

**Rationale**: Two structural differences make the shared trait a poor fit: (1) Anthropic's system prompt is a top-level `system` field in the request body, not a message in the conversation array; (2) thinking blocks must be filtered from outgoing requests because the API rejects them. The shared `MessageConverter` trait assumes system prompt is a message and does not have a filtering step.

**Alternatives rejected**:
- *Shared `MessageConverter` with overrides*: Would require adding system-prompt-extraction and block-filtering hooks to the trait, complicating the interface for all other adapters that don't need them.
- *Post-processing filter after shared conversion*: Two-pass approach; less clear than a single purpose-built function.

## Decision 4: Error Classification with 529 Handling

**Question**: How should HTTP errors from the Anthropic API be classified?

**Decision**: Inline match in `anthropic_stream()` mapping status codes to `AssistantMessageEvent` error constructors: 401 -> `error_auth`, 429 -> `error_throttled`, 529 -> `error_network` (retryable), 504 -> `error_network`, 400-499 -> `error` (generic, not retryable), 500-599 -> `error_network` (retryable). The shared `classify_http_status` is not used directly because the adapter constructs `AssistantMessageEvent` variants inline with provider-specific error messages.

**Rationale**: The 529 (overloaded) status code is Anthropic-specific and maps to a retryable network error per the spec clarifications. The inline match gives full control over error message formatting (including the response body) while still mapping to the same error kinds the shared classifier would produce.

**Alternatives rejected**:
- *Use `classify_with_overrides` and a separate event-construction step*: Adds indirection for the same outcome; the inline match is clearer and more direct.

## Decision 5: Index Remapping for Filtered Blocks

**Question**: How should content block indices be managed when thinking blocks are present?

**Decision**: `SseStreamState` maintains a monotonically increasing `content_index` counter and a `HashMap<usize, (BlockType, usize)>` mapping Anthropic's block indices to `(block_type, harness_content_index)` pairs. Each `content_block_start` allocates the next `content_index` regardless of block type. Deltas and stops look up the harness index from the map.

**Rationale**: Anthropic's block indices include thinking blocks, but the harness content indices must be contiguous. The HashMap provides O(1) lookup and naturally handles the remapping. Blocks are removed from the map on `content_block_stop`, enabling the `StreamFinalize` trait to drain any blocks still open at stream end.

**Alternatives rejected**:
- *Direct use of Anthropic indices*: Would leave gaps in the harness content index sequence when thinking blocks are present.
- *Offset arithmetic*: Fragile; would need to track how many thinking blocks preceded each content block.
