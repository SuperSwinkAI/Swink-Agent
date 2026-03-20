# Data Model: Adapter: Proxy

**Feature**: 020-adapter-proxy | **Date**: 2026-03-20

## Entity: ProxyStreamFn (struct, public)

**Location**: `adapters/src/proxy.rs`

| Field | Type | Purpose |
|-------|------|---------|
| `base_url` | `String` | Base URL of the proxy server (without trailing slash) |
| `bearer_token` | `String` | Bearer token for authentication |
| `client` | `reqwest::Client` | Shared HTTP client for connection pooling |

| Method | Signature | Purpose |
|--------|-----------|---------|
| `new()` | `fn(impl Into<String>, impl Into<String>) -> Self` | Primary constructor: base_url and bearer_token |

**Traits implemented**:
- `StreamFn` — `stream()` sends JSON POST to `{base_url}/v1/stream` and parses SSE response
- `Debug` — redacts `bearer_token` as `[redacted]`
- `Send + Sync` — compile-time assertion via `const` block

**Re-export**: `pub use proxy::ProxyStreamFn` in `adapters/src/lib.rs`

---

## Entity: SseEventData (enum, private)

**Location**: `adapters/src/proxy.rs`

Deserialized from the `data:` field of each SSE event. Tagged union via `#[serde(tag = "type", rename_all = "snake_case")]`.

| Variant | Fields | Maps to |
|---------|--------|---------|
| `Start` | — | `AssistantMessageEvent::Start` |
| `TextStart` | `content_index: usize` | `AssistantMessageEvent::TextStart` |
| `TextDelta` | `content_index: usize`, `delta: String` | `AssistantMessageEvent::TextDelta` |
| `TextEnd` | `content_index: usize` | `AssistantMessageEvent::TextEnd` |
| `ThinkingStart` | `content_index: usize` | `AssistantMessageEvent::ThinkingStart` |
| `ThinkingDelta` | `content_index: usize`, `delta: String` | `AssistantMessageEvent::ThinkingDelta` |
| `ThinkingEnd` | `content_index: usize`, `signature: Option<String>` | `AssistantMessageEvent::ThinkingEnd` |
| `ToolCallStart` | `content_index: usize`, `id: String`, `name: String` | `AssistantMessageEvent::ToolCallStart` |
| `ToolCallDelta` | `content_index: usize`, `delta: String` | `AssistantMessageEvent::ToolCallDelta` |
| `ToolCallEnd` | `content_index: usize` | `AssistantMessageEvent::ToolCallEnd` |
| `Done` | `stop_reason: StopReason`, `usage: Usage`, `cost: Cost` | `AssistantMessageEvent::Done` |
| `Error` | `stop_reason: StopReason`, `error_message: String`, `usage: Option<Usage>` | `AssistantMessageEvent::Error` |

**Derives**: `Deserialize`

---

## Entity: ProxyRequest (struct, private)

**Location**: `adapters/src/proxy.rs`

JSON body sent to the proxy endpoint via POST.

| Field | Type | Purpose |
|-------|------|---------|
| `model` | `&'a str` | Model ID from `ModelSpec` |
| `system` | `&'a str` | System prompt from `AgentContext` |
| `messages` | `Vec<&'a LlmMessage>` | LLM messages (CustomMessage filtered out) |
| `options` | `ProxyRequestOptions<'a>` | Forwarded options |

**Derives**: `Serialize`
**Lifetime**: `'a` — borrows from `ModelSpec`, `AgentContext`, and `StreamOptions`

---

## Entity: ProxyRequestOptions (struct, private)

**Location**: `adapters/src/proxy.rs`

Options subset forwarded to the proxy.

| Field | Type | Serialization | Purpose |
|-------|------|---------------|---------|
| `temperature` | `Option<f64>` | skip_serializing_if None | Sampling temperature |
| `max_tokens` | `Option<u64>` | skip_serializing_if None | Max output tokens |
| `session_id` | `Option<&'a str>` | skip_serializing_if None | Session identifier for proxy routing |

**Derives**: `Serialize`

---

## Relationship Diagram

```text
ProxyStreamFn (public)
  │
  ├── implements StreamFn::stream()
  │     │
  │     ├── send_request() ──► ProxyRequest + ProxyRequestOptions (serialized to JSON)
  │     │     │
  │     │     └── bearer_auth(token) ──► Authorization header
  │     │
  │     ├── classify_response_status() ──► classify_http_status() from crate::classify
  │     │
  │     └── parse_sse_stream()
  │           │
  │           ├── eventsource_stream::Eventsource ──► SSE event stream
  │           │
  │           ├── parse_sse_event_data() ──► serde_json::from_str::<SseEventData>()
  │           │
  │           └── convert_sse_event() ──► AssistantMessageEvent (1:1 mapping)
  │
  └── Debug ──► redacts bearer_token
```
