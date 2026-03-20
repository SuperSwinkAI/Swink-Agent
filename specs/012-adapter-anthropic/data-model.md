# Data Model: Adapter: Anthropic

**Feature**: 012-adapter-anthropic | **Date**: 2026-03-20

## Entity: AnthropicStreamFn (public struct)

**Location**: `adapters/src/anthropic.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `base` | `AdapterBase` | Shared HTTP client, base URL, and API key |

| Method/Trait Impl | Signature | Purpose |
|-------------------|-----------|---------|
| `new(base_url, api_key)` | `pub fn new(impl Into<String>, impl Into<String>) -> Self` | Primary constructor |
| `StreamFn::stream()` | `fn stream(&self, model, context, options, token) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent>>>` | Entry point for streaming |
| `Debug::fmt()` | Standard | Redacts API key in debug output |

**Compile-time assertion**: `AnthropicStreamFn: Send + Sync`

---

## Entity: AnthropicContentBlock (private enum, serializable)

**Location**: `adapters/src/anthropic.rs`

| Variant | Fields | Serialized `type` | Purpose |
|---------|--------|--------------------|---------|
| `Text` | `text: String` | `"text"` | Text content in outgoing messages |
| `ToolUse` | `id: String, name: String, input: Value` | `"tool_use"` | Tool call in outgoing assistant messages |
| `ToolResult` | `tool_use_id: String, content: String` | `"tool_result"` | Tool result in outgoing user messages |

**Note**: Thinking blocks are intentionally absent -- the Anthropic API rejects thinking blocks in outgoing requests.

---

## Entity: AnthropicMessage (private struct, serializable)

**Location**: `adapters/src/anthropic.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `role` | `String` | `"user"` or `"assistant"` |
| `content` | `Vec<AnthropicContentBlock>` | Content blocks for this message |

---

## Entity: AnthropicThinking (private struct, serializable)

**Location**: `adapters/src/anthropic.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `type` | `String` | Always `"enabled"` |
| `budget_tokens` | `u64` | Maximum tokens for model reasoning; must be < `max_tokens` |

---

## Entity: AnthropicChatRequest (private struct, serializable)

**Location**: `adapters/src/anthropic.rs`

| Field | Type | Serialization | Purpose |
|-------|------|---------------|---------|
| `model` | `String` | Always | Model identifier |
| `max_tokens` | `u64` | Always | Maximum output tokens |
| `stream` | `bool` | Always | Always `true` |
| `system` | `Option<String>` | Skip if None | System prompt (top-level, not a message) |
| `messages` | `Vec<AnthropicMessage>` | Always | Conversation messages |
| `tools` | `Vec<AnthropicToolDef>` | Skip if empty | Tool definitions |
| `temperature` | `Option<f64>` | Skip if None | Forced to None when thinking is enabled |
| `thinking` | `Option<AnthropicThinking>` | Skip if None | Extended thinking configuration |

---

## Entity: BlockType (private enum)

**Location**: `adapters/src/anthropic.rs`

| Variant | Purpose |
|---------|---------|
| `Text` | Text content block |
| `Thinking` | Thinking/reasoning block |
| `ToolUse` | Tool call block |

---

## Entity: SseStreamState (private struct)

**Location**: `adapters/src/anthropic.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `content_index` | `usize` | Next harness content index to allocate |
| `active_blocks` | `HashMap<usize, (BlockType, usize)>` | Maps Anthropic block index to (type, harness index) |
| `usage` | `Usage` | Accumulated token usage (input, output, cache) |
| `stop_reason` | `Option<StopReason>` | Stop reason from `message_delta` event |

**Implements**: `StreamFinalize` (via `drain_open_blocks`) for clean block closure on cancellation or unexpected end.

---

## Entity: SseLine (private enum)

**Location**: `adapters/src/anthropic.rs`

| Variant | Fields | Purpose |
|---------|--------|---------|
| `Event` | `event_type: String, data: String` | Paired `event:` + `data:` SSE lines |

---

## Relationship Diagram

```text
AnthropicStreamFn
  └── base: AdapterBase
        ├── base_url: String
        ├── api_key: String
        └── client: reqwest::Client

StreamFn::stream()
  ├── send_request()
  │     ├── convert_messages() → (Option<String>, Vec<AnthropicMessage>)
  │     ├── extract_tool_schemas() → Vec<AnthropicToolDef>
  │     ├── resolve_thinking() → Option<AnthropicThinking>
  │     └── POST /v1/messages with AnthropicChatRequest
  │
  └── parse_sse_stream()
        ├── sse_event_lines() → Stream<Item = SseLine>
        └── process_sse_event() → Vec<AssistantMessageEvent>
              └── SseStreamState (tracks active_blocks, usage, stop_reason)
                    └── impl StreamFinalize (drain_open_blocks for finalization)
```
