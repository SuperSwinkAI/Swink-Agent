# Data Model: Adapter: Ollama

**Feature**: 014-adapter-ollama | **Date**: 2026-03-20

## Entity: OllamaStreamFn (public struct)

**Location**: `adapters/src/ollama.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `base_url` | `String` | Ollama server URL (e.g. `http://localhost:11434`) |
| `client` | `reqwest::Client` | HTTP client for requests |

| Method/Trait Impl | Signature | Purpose |
|-------------------|-----------|---------|
| `new(base_url)` | `pub fn new(impl Into<String>) -> Self` | Primary constructor |
| `StreamFn::stream()` | `fn stream(&self, model, context, options, token) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent>>>` | Entry point for streaming |
| `Debug::fmt()` | Standard | Shows base_url, omits client internals |

**Compile-time assertion**: `OllamaStreamFn: Send + Sync`

---

## Entity: OllamaChatChunk (private struct, deserializable)

**Location**: `adapters/src/ollama.rs`

| Field | Type | Serde | Purpose |
|-------|------|-------|---------|
| `message` | `OllamaResponseMessage` | Required | The message content for this chunk |
| `done` | `bool` | Required | Whether this is the final chunk |
| `done_reason` | `Option<String>` | `#[serde(default)]` | Terminal reason: `"stop"`, `"tool_calls"`, `"length"` |
| `prompt_eval_count` | `Option<u64>` | `#[serde(default)]` | Input token count (present only in final chunk) |
| `eval_count` | `Option<u64>` | `#[serde(default)]` | Output token count (present only in final chunk) |

---

## Entity: OllamaResponseMessage (private struct, deserializable)

**Location**: `adapters/src/ollama.rs`

| Field | Type | Serde | Purpose |
|-------|------|-------|---------|
| `content` | `String` | `#[serde(default)]` | Text content fragment (empty string if absent) |
| `thinking` | `Option<String>` | `#[serde(default)]` | Thinking/reasoning content (model-dependent) |
| `tool_calls` | `Option<Vec<OllamaResponseToolCall>>` | `#[serde(default)]` | Complete tool calls (not deltas) |

---

## Entity: OllamaResponseToolCall (private struct, deserializable)

**Location**: `adapters/src/ollama.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `function` | `OllamaResponseFunction` | Function name and arguments |

**Sub-entity: OllamaResponseFunction**

| Field | Type | Purpose |
|-------|------|---------|
| `name` | `String` | Tool function name |
| `arguments` | `serde_json::Value` | Complete JSON arguments (not fragmented) |

---

## Entity: StreamState (private struct)

**Location**: `adapters/src/ollama.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `text_started` | `bool` | Whether a text block is currently open |
| `thinking_started` | `bool` | Whether a thinking block is currently open |
| `content_index` | `usize` | Next harness content index to allocate |
| `tool_calls_started` | `HashSet<String>` | Tracks tool names seen to deduplicate across chunks |

**Implements**: `StreamFinalize` (via `drain_open_blocks`) for clean block closure on cancellation or unexpected stream end. Drains open thinking blocks first, then open text blocks.

---

## Relationship Diagram

```text
OllamaStreamFn
  ├── base_url: String
  └── client: reqwest::Client

StreamFn::stream()
  ├── send_request()
  │     ├── convert_messages::<OllamaConverter>() → Vec<OllamaMessage>
  │     ├── extract_tool_schemas() → Vec<OllamaTool>
  │     └── POST /api/chat with OllamaChatRequest
  │
  └── parse_ndjson_stream()
        ├── ndjson_lines() → Stream<Item = String>  (custom NDJSON parser)
        └── stream::unfold with StreamState
              ├── process thinking content → ThinkingStart/ThinkingDelta/ThinkingEnd
              ├── process text content → TextStart/TextDelta/TextEnd
              ├── process tool calls → ToolCallStart/ToolCallDelta/ToolCallEnd
              └── done: true → finalize_blocks() + Done event
                    └── impl StreamFinalize (drain_open_blocks for finalization)
```
