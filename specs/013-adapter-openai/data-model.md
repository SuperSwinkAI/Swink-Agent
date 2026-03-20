# Data Model: Adapter: OpenAI

**Feature**: 013-adapter-openai | **Date**: 2026-03-20

## Entity: OpenAiStreamFn (public struct)

**Location**: `adapters/src/openai.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `base` | `AdapterBase` | Shared HTTP client, base URL, and API key |

| Method/Trait Impl | Signature | Purpose |
|-------------------|-----------|---------|
| `new(base_url, api_key)` | `pub fn new(impl Into<String>, impl Into<String>) -> Self` | Primary constructor |
| `StreamFn::stream()` | `fn stream(&self, model, context, options, token) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent>>>` | Entry point for streaming |
| `Debug::fmt()` | Standard | Redacts API key in debug output |

**Compile-time assertion**: `OpenAiStreamFn: Send + Sync`

---

## Entity: OaiToolCallDelta (shared struct, deserializable)

**Location**: `adapters/src/openai_compat.rs`

| Field | Type | Serde | Purpose |
|-------|------|-------|---------|
| `index` | `usize` | Required | Identifies which parallel tool call this delta belongs to |
| `id` | `Option<String>` | `#[serde(default)]` | Tool call ID (present on first delta; absent on subsequent deltas and some providers) |
| `function` | `Option<OaiFunctionDelta>` | `#[serde(default)]` | Function name and/or argument fragment |

**Note**: Used by OpenAI, Azure, Mistral, and xAI adapters.

---

## Entity: ToolCallState (shared struct)

**Location**: `adapters/src/openai_compat.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `arguments` | `String` | Accumulated JSON argument fragments |
| `started` | `bool` | Whether `ToolCallStart` has been emitted |
| `content_index` | `usize` | Harness content index assigned to this tool call |

---

## Entity: SseStreamState (private struct)

**Location**: `adapters/src/openai.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `text_started` | `bool` | Whether a text block is currently open |
| `content_index` | `usize` | Next harness content index to allocate |
| `tool_calls` | `HashMap<usize, ToolCallState>` | Maps tool call index to accumulated state |
| `usage` | `Option<Usage>` | Token usage captured from the usage chunk |
| `stop_reason` | `Option<StopReason>` | Saved from `finish_reason`; emitted with `Done` |

**Implements**: `StreamFinalize` (via `drain_open_blocks`) for clean block closure on cancellation or unexpected stream end. Drains open text blocks first, then open tool calls sorted by index.

---

## Entity: OaiChatChunk (OaiChunk -- shared struct, deserializable)

**Location**: `adapters/src/openai_compat.rs`

| Field | Type | Serde | Purpose |
|-------|------|-------|---------|
| `choices` | `Vec<OaiChoice>` | `#[serde(default)]` | Array of choice deltas (typically one) |
| `usage` | `Option<OaiUsage>` | `#[serde(default)]` | Token usage (arrives in final chunk when `include_usage: true`) |

**Sub-entity: OaiChoice**

| Field | Type | Serde | Purpose |
|-------|------|-------|---------|
| `delta` | `OaiDelta` | `#[serde(default)]` | Content delta (text and/or tool calls) |
| `finish_reason` | `Option<String>` | `#[serde(default)]` | Terminal reason: `"stop"`, `"tool_calls"`, `"length"`, `"content_filter"` |

**Sub-entity: OaiDelta**

| Field | Type | Serde | Purpose |
|-------|------|-------|---------|
| `content` | `Option<String>` | `#[serde(default)]` | Text content fragment |
| `tool_calls` | `Option<Vec<OaiToolCallDelta>>` | `#[serde(default)]` | Tool call delta fragments |

---

## Relationship Diagram

```text
OpenAiStreamFn
  └── base: AdapterBase
        ├── base_url: String
        ├── api_key: String
        └── client: reqwest::Client

StreamFn::stream()
  ├── send_request()
  │     ├── convert_messages::<OaiConverter>() → Vec<OaiMessage>
  │     ├── build_oai_tools() → (Vec<OaiTool>, Option<String>)
  │     └── POST /v1/chat/completions with OaiChatRequest
  │
  └── parse_sse_stream()
        ├── sse_data_lines() → Stream<Item = SseLine>  (shared SSE parser)
        └── stream::unfold with SseStreamState
              ├── process text deltas → TextStart/TextDelta/TextEnd
              ├── process_tool_call_delta() → ToolCallStart/ToolCallDelta
              ├── finish_reason → finalize_blocks() + save StopReason
              └── [DONE] or stream end → Done event
                    └── impl StreamFinalize (drain_open_blocks for finalization)
```
