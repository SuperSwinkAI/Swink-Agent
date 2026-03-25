# Data Model: Adapter: Google Gemini

**Feature**: 015-adapter-gemini | **Date**: 2026-03-24

## Entity: GeminiStreamFn (public struct)

**Location**: `adapters/src/google.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `base_url` | `String` | Generative Language API base URL (trailing `/` stripped) |
| `api_key` | `String` | API key for `x-goog-api-key` header |
| `api_version` | `ApiVersion` | `V1` or `V1beta` — embedded in URL path |
| `client` | `reqwest::Client` | Shared HTTP client |

| Method/Trait Impl | Signature | Purpose |
|-------------------|-----------|---------|
| `new(base_url, api_key, api_version)` | `pub fn new(impl Into<String>, impl Into<String>, ApiVersion) -> Self` | Primary constructor |
| `api_version_path()` | `const fn api_version_path(&self) -> &'static str` | Returns `"v1"` or `"v1beta"` |
| `StreamFn::stream()` | `fn stream(&self, model, context, options, token) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent>>>` | Entry point for streaming |
| `Debug::fmt()` | Standard | Redacts API key in debug output |

**Compile-time assertion**: `GeminiStreamFn: Send + Sync`

---

## Entity: GeminiRequest (private struct, serializable)

**Location**: `adapters/src/google.rs`

| Field | Type | Serialization | Purpose |
|-------|------|---------------|---------|
| `system_instruction` | `Option<GeminiContent>` | Skip if None | System prompt as a content object |
| `contents` | `Vec<GeminiContent>` | Always | Conversation messages |
| `tools` | `Vec<GeminiTool>` | Skip if empty | Function declaration wrappers |
| `tool_config` | `Option<GeminiToolConfig>` | Skip if None | Function calling mode (`"AUTO"`) |
| `generation_config` | `Option<GeminiGenerationConfig>` | Skip if None | Temperature, max tokens, thinking config |

---

## Entity: GeminiContent (private struct, serializable)

**Location**: `adapters/src/google.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `role` | `String` | `"user"` or `"model"` |
| `parts` | `Vec<GeminiPart>` | Content parts (text, images, function calls/responses) |

---

## Entity: GeminiPart (private struct, serializable)

**Location**: `adapters/src/google.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `text` | `Option<String>` | Text content |
| `thought` | `Option<bool>` | `true` for thinking blocks |
| `thought_signature` | `Option<String>` | Signature for thinking block continuity |
| `inline_data` | `Option<GeminiInlineData>` | Base64-encoded image data |
| `file_data` | `Option<GeminiFileData>` | File URI reference |
| `function_call` | `Option<GeminiFunctionCall>` | Function call in assistant messages |
| `function_response` | `Option<GeminiFunctionResponse>` | Function result in user messages |

---

## Entity: GeminiInlineData (private struct, serializable)

**Location**: `adapters/src/google.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `mime_type` | `String` | Image MIME type (e.g., `"image/png"`) |
| `data` | `String` | Base64-encoded image data |

---

## Entity: GeminiFunctionCall (private struct, serializable + deserializable)

**Location**: `adapters/src/google.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `id` | `Option<String>` | Provider-assigned call ID (may be absent) |
| `name` | `String` | Function name |
| `args` | `Value` | Function arguments as JSON |

---

## Entity: GeminiFunctionResponse (private struct, serializable)

**Location**: `adapters/src/google.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `name` | `String` | Function name (must match the call) |
| `response` | `Value` | Tool result as JSON |

---

## Entity: GeminiChunk (private struct, deserializable)

**Location**: `adapters/src/google.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `candidates` | `Vec<GeminiCandidate>` | Response candidates (only first is used) |
| `usage_metadata` | `Option<GeminiUsageMetadata>` | Token usage counts |

---

## Entity: GeminiCandidate (private struct, deserializable)

**Location**: `adapters/src/google.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `content` | `Option<GeminiResponseContent>` | Content parts in the response |
| `finish_reason` | `Option<String>` | `"STOP"`, `"MAX_TOKENS"`, `"SAFETY"`, etc. |

---

## Entity: GeminiStreamState (private struct)

**Location**: `adapters/src/google.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `text_started` | `bool` | Whether a text block is currently open |
| `text_content_index` | `Option<usize>` | Harness content index for current text block |
| `thinking_started` | `bool` | Whether a thinking block is currently open |
| `thinking_content_index` | `Option<usize>` | Harness content index for current thinking block |
| `thinking_signature` | `Option<String>` | Buffered signature for thinking block end |
| `next_content_index` | `usize` | Next harness content index to allocate |
| `tool_calls` | `HashMap<usize, GeminiToolCallState>` | Active tool calls keyed by part index |
| `saw_tool_call` | `bool` | Whether any tool call was seen (for stop reason) |
| `usage` | `Usage` | Accumulated token usage |
| `stop_reason` | `Option<StopReason>` | Stop reason from finish_reason |

**Implements**: `StreamFinalize` (via `drain_open_blocks`) for clean block closure on cancellation or unexpected end.

---

## Entity: GeminiToolCallState (private struct)

**Location**: `adapters/src/google.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `id` | `String` | Tool call ID (provider-assigned or generated) |
| `name` | `String` | Function name |
| `content_index` | `usize` | Harness content index |
| `arguments` | `String` | Accumulated serialized arguments (for delta computation) |

---

## Relationship Diagram

```text
GeminiStreamFn
  ├── base_url: String
  ├── api_key: String
  ├── api_version: ApiVersion
  └── client: reqwest::Client

StreamFn::stream()
  ├── send_request()
  │     ├── convert_messages() → Vec<GeminiContent>
  │     ├── build_tools() → Vec<GeminiTool>
  │     └── POST /{version}/models/{model}:streamGenerateContent?alt=sse
  │
  └── parse_sse_stream()
        ├── sse_data_lines() → Stream<Item = SseLine>
        └── process_chunk() → Vec<AssistantMessageEvent>
              ├── process_function_call() (tool call events)
              ├── map_finish_reason() (stop reason mapping)
              └── GeminiStreamState (tracks text/thinking/tool blocks, usage)
                    └── impl StreamFinalize (drain_open_blocks for finalization)
```
