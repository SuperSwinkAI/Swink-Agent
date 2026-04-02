# Data Model: Adapter AWS Bedrock

**Feature**: 019-adapter-bedrock | **Date**: 2026-04-02

## Entities

### BedrockStreamFn (public)

The streaming function that connects to AWS Bedrock's ConverseStream endpoint.

| Field | Type | Description |
|-------|------|-------------|
| `base_url` | `String` | `https://bedrock-runtime.{region}.amazonaws.com` |
| `region` | `String` | AWS region (e.g., `us-east-1`) |
| `access_key_id` | `String` | AWS access key ID |
| `secret_access_key` | `String` | AWS secret access key |
| `session_token` | `Option<String>` | Optional session token for temporary credentials |
| `client` | `reqwest::Client` | HTTP client for requests |

**Constructors**:
- `new(region, access_key_id, secret_access_key, session_token)` — derives `base_url` from region
- `new_with_base_url(base_url, region, ...)` — for testing or custom endpoints

**Trait implementations**: `StreamFn`, `Debug` (redacted credentials), `Send + Sync`

### BedrockRequest (internal, serialized)

Request body for ConverseStream endpoint.

| Field | Type | Description |
|-------|------|-------------|
| `messages` | `Vec<BedrockMessage>` | Conversation messages |
| `system` | `Option<Vec<BedrockSystemBlock>>` | System prompt (top-level, not synthetic user msg) |
| `inferenceConfig` | `Option<BedrockInferenceConfig>` | Temperature, max_tokens |
| `toolConfig` | `Option<BedrockToolConfig>` | Tool definitions |

### BedrockMessage (internal, serialized)

| Field | Type | Description |
|-------|------|-------------|
| `role` | `String` | `"user"` or `"assistant"` |
| `content` | `Vec<BedrockContentBlock>` | Text, tool_use, or tool_result blocks |

### Streaming Event Types (internal, deserialized from event-stream payloads)

| Event Type | Key Fields | Maps To |
|------------|------------|---------|
| `MessageStartEvent` | `role: String` | `AssistantMessageEvent::Start` |
| `ContentBlockStartEvent` | `contentBlockIndex: usize`, `start: StartBlock` | `TextStart` or `ToolCallStart` |
| `ContentBlockDeltaEvent` | `contentBlockIndex: usize`, `delta: DeltaBlock` | `TextDelta` or `ToolCallDelta` |
| `ContentBlockStopEvent` | `contentBlockIndex: usize` | `TextEnd` or `ToolCallEnd` |
| `MessageStopEvent` | `stopReason: String` | (captured for Done event) |
| `MetadataEvent` | `usage: BedrockUsage`, `metrics: Metrics` | `AssistantMessageEvent::Done` |

### StartBlock (internal, deserialized)

Tagged union for `contentBlockStart.start`:
- Text variant: `{ "type": "text" }` (no additional fields at start)
- ToolUse variant: `{ "type": "toolUse", "toolUseId": String, "name": String }`

### DeltaBlock (internal, deserialized)

Tagged union for `contentBlockDelta.delta`:
- Text variant: `{ "type": "text", "text": String }`
- ToolUse variant: `{ "type": "toolUse", "input": String }` (partial JSON fragment)

## Relationships

```
BedrockStreamFn
  --signs-with--> SigV4 (hmac/sha2 crypto helpers)
  --sends-to--> POST /model/{modelId}/converse-stream
  --receives--> application/vnd.amazon.eventstream binary frames
  --parses-via--> aws-smithy-eventstream::MessageFrameDecoder
  --deserializes--> Streaming Event Types (JSON payloads)
  --emits--> AssistantMessageEvent stream
```

## State Machine (Streaming)

```
[Request Sent] → [messageStart] → [contentBlockStart]
                                        ↓
                                   [contentBlockDelta]* (repeats)
                                        ↓
                                   [contentBlockStop]
                                        ↓
                                   [contentBlockStart] (next block, if any)
                                        ...
                                   [messageStop] → [metadata] → DONE
```

State tracked during streaming:
- `current_block_type: Option<BlockType>` — Text or ToolUse (set at contentBlockStart, cleared at contentBlockStop)
- `stop_reason: Option<StopReason>` — captured from messageStop
- `decoder: MessageFrameDecoder` — incremental binary frame decoder

## Validation Rules

- `region` must not be empty (Bedrock requires a valid AWS region)
- `access_key_id` and `secret_access_key` must not be empty
- `model_id` passed through to URL path (Bedrock validates server-side)
- Event-stream frames validated by CRC (handled by aws-smithy-eventstream)
