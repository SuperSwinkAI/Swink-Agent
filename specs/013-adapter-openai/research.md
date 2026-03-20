# Research: Adapter: OpenAI

**Feature**: 013-adapter-openai | **Date**: 2026-03-20

## Decision 1: SSE Protocol with `[DONE]` Sentinel

**Question**: How should the adapter parse the OpenAI SSE streaming format?

**Decision**: Use the shared `sse_data_lines()` combinator from `sse.rs`, which buffers incoming bytes through `SseStreamParser` and yields `SseLine::Data` and `SseLine::Done` variants. The `[DONE]` sentinel signals stream termination. The `stream::unfold` state machine processes each `SseLine` and emits `AssistantMessageEvent` values.

**Rationale**: Unlike Anthropic (which needs `event:` type labels for state machine dispatch), the OpenAI SSE format carries all information in the `data:` payload JSON. The event type is not needed because the chunk structure itself indicates whether a delta contains text, tool calls, or a finish reason. The shared `sse_data_lines()` combinator strips event types and comments, which is exactly what the OpenAI adapter needs.

**Alternatives rejected**:
- *Custom SSE line parser (like Anthropic)*: Unnecessary complexity; OpenAI does not use `event:` labels for dispatch.
- *`eventsource-stream` crate*: Adds a dependency for functionality already provided by the shared parser.

## Decision 2: Multi-Provider Compatibility via Base URL

**Question**: How should the adapter support alternative OpenAI-compatible providers?

**Decision**: The adapter accepts a configurable `base_url` parameter and appends `/v1/chat/completions` to form the endpoint URL. All JSON parsing uses `#[serde(default)]` on optional fields so that providers that omit fields (e.g., `usage`, `tool_calls`, `finish_reason`) do not cause parse errors. No provider-specific code paths exist.

**Rationale**: The OpenAI chat completions SSE protocol is a de facto standard. Providers like vLLM, LM Studio, Groq, and Together implement it with minor variations (missing fields, different finish reasons, absent tool call IDs). Lenient parsing via `#[serde(default)]` absorbs these differences without branching. This matches the spec's assumption that "provider-specific quirks are handled by being lenient in parsing, not by provider-specific code paths."

**Alternatives rejected**:
- *Provider enum with per-provider adjustments*: Violates the protocol-not-provider design principle; would require updates for every new provider.
- *Strict parsing with error recovery*: Would produce spurious errors on valid-but-sparse responses from alternative providers.

## Decision 3: Tool Call Delta Processing with UUID Fallback

**Question**: How should tool call deltas be tracked and assembled, especially when provider omits the tool call ID?

**Decision**: Tool call state is tracked in a `HashMap<usize, ToolCallState>` keyed by the tool call index from the SSE chunk. On first encounter (vacant entry), the adapter extracts the `id` and `name` from the delta; if `id` is absent, it auto-generates one as `tc_{uuid}`. Subsequent deltas for the same index append to the accumulated `arguments` string. On `finish_reason`, all open tool calls are finalized via `StreamFinalize::drain_open_blocks()`.

**Rationale**: OpenAI sends tool call deltas with an `index` field that identifies which parallel tool call a delta belongs to. The first delta for an index carries the `id` and `name`; subsequent deltas carry only argument fragments. Some alternative providers omit the `id` field entirely. The UUID fallback ensures the harness always has a unique tool call identifier for dispatch, and the `tc_` prefix makes it visually identifiable as auto-generated.

**Alternatives rejected**:
- *Error on missing tool call ID*: Would break compatibility with providers like vLLM that sometimes omit IDs.
- *Sequential integer IDs*: Less unique; could collide across turns in a multi-turn conversation.

## Decision 4: Lenient Parsing for Alternative Providers

**Question**: How should the adapter handle structural variations across OpenAI-compatible providers?

**Decision**: All response types use `#[serde(default)]` on optional and collection fields. Empty `choices` arrays are iterated without error (no-op). Missing `finish_reason` is handled at stream end (either from a prior chunk or via `StopReason::Stop` default). Missing `usage` defaults to zero. The `OaiChunk`, `OaiChoice`, `OaiDelta`, `OaiToolCallDelta`, and `OaiFunctionDelta` structs all derive `Deserialize` with defaults on every optional field.

**Rationale**: The spec explicitly calls out several edge cases (empty choices, missing tool call IDs, missing `[DONE]`) and requires graceful handling. Using `#[serde(default)]` is the idiomatic Rust/serde approach and makes the adapter naturally tolerant of sparse JSON without explicit null checks.

**Alternatives rejected**:
- *Manual JSON parsing with `serde_json::Value`*: Loses type safety and is more verbose.
- *Strict deserialization with error mapping*: Would reject valid responses from providers that use a subset of the schema.

## Decision 5: Finish Reason Mapping

**Question**: How should the adapter map OpenAI's `finish_reason` strings to `StopReason`?

**Decision**: A wildcard match maps known reasons and defaults unknown ones to `StopReason::Stop`:
- `"tool_calls"` -> `StopReason::ToolUse`
- `"length"` -> `StopReason::Length`
- `"stop"` | `"content_filter"` | any other value -> `StopReason::Stop`

The finish reason is saved in `SseStreamState.stop_reason` when received (which may be in a chunk before `[DONE]`), and emitted with the `Done` event when `[DONE]` arrives or the stream ends.

**Rationale**: The spec clarifies that unrecognized `finish_reason` values should default to `StopReason::Stop`. This wildcard approach is future-proof -- new finish reasons from OpenAI or alternative providers will not cause errors. The deferred emission (save on `finish_reason`, emit on `[DONE]`) handles the common OpenAI pattern where usage data arrives in a chunk after the `finish_reason` chunk.

**Alternatives rejected**:
- *Error on unrecognized finish_reason*: Would break with new providers or API versions that introduce new reasons.
- *Separate `StopReason::ContentFilter` variant*: Not present in the core `StopReason` enum; `Stop` is the appropriate catch-all.
