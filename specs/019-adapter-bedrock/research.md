# Research: Adapter AWS Bedrock

**Feature**: 019-adapter-bedrock | **Date**: 2026-04-02

## R1: ConverseStream API Protocol

**Decision**: Use the `ConverseStream` endpoint (`POST /model/{modelId}/converse-stream`) with AWS event-stream binary encoding.

**Rationale**: Bedrock's ConverseStream API delivers true incremental streaming via `application/vnd.amazon.eventstream` binary framing. The request body is identical to the non-streaming `Converse` API (same JSON format). The response is a sequence of binary event-stream frames, each containing a JSON payload with one of 6 event types: `messageStart`, `contentBlockStart`, `contentBlockDelta`, `contentBlockStop`, `messageStop`, `metadata`.

**Alternatives considered**:
- Non-streaming `Converse` API (existing stub) → rejected (buffers entire response, violates SC-001)
- `InvokeModelWithResponseStream` → rejected (model-specific request formats, ConverseStream is the unified API)

## R2: AWS Event-Stream Binary Protocol

**Decision**: Use `aws-smithy-eventstream` crate (v0.60.x) + `aws-smithy-types` for binary frame parsing.

**Rationale**: AWS event-stream is a binary framed protocol (not SSE). Each frame has: `[total_length: u32][headers_length: u32][prelude_crc: u32][headers...][payload...][message_crc: u32]`. The `aws-smithy-eventstream::frame::MessageFrameDecoder` handles incremental decoding: feed bytes, get `DecodedFrame::Complete(Message)` or `DecodedFrame::Incomplete`. Key headers per frame: `:message-type` (event/exception), `:event-type` (messageStart, contentBlockDelta, etc.), `:content-type` (application/json).

**Alternatives considered**:
- Hand-rolled binary parser → rejected (complex protocol with CRC validation, error-prone)
- Full AWS SDK (`aws-sdk-bedrockruntime`) → rejected (massive dependency tree for just event-stream parsing)

## R3: ConverseStream Event Types

**Decision**: Map Bedrock streaming events to `AssistantMessageEvent` as follows:

| Bedrock Event | Harness Event | Notes |
|---|---|---|
| `messageStart` | `Start` | Contains `role: "assistant"` |
| `contentBlockStart` (text) | `TextStart` | `contentBlockIndex` → `content_index` |
| `contentBlockStart` (toolUse) | `ToolCallStart` | Contains `toolUseId`, `name` |
| `contentBlockDelta` (text) | `TextDelta` | `delta.text` → `delta` |
| `contentBlockDelta` (toolUse) | `ToolCallDelta` | `delta.input` → `delta` (partial JSON string) |
| `contentBlockStop` | `TextEnd` or `ToolCallEnd` | Tracked by content block type |
| `messageStop` | (internal) | Captures `stopReason` |
| `metadata` | `Done` | Contains `usage`, triggers terminal event |

**Rationale**: Direct 1:1 mapping from Bedrock streaming events to harness events. Tool call arguments arrive as incremental string fragments in `delta.input` and are emitted as `ToolCallDelta` events.

## R4: Stop Reason Mapping

**Decision**: Map Bedrock stop reasons as follows:

| Bedrock `stopReason` | Harness `StopReason` |
|---|---|
| `end_turn` | `Stop` |
| `stop_sequence` | `Stop` |
| `tool_use` | `ToolUse` |
| `max_tokens` | `Length` |
| `guardrail_intervened` | → `ContentFiltered` error event |

**Rationale**: Consistent with existing adapter patterns. `guardrail_intervened` is special-cased to emit an error event rather than a normal Done, matching the Azure adapter's `ContentFiltered` handling per clarification.

## R5: System Prompt Handling

**Decision**: Use Bedrock's native `system` field in the ConverseStream request body instead of prepending a synthetic user message.

**Rationale**: The existing stub prepends the system prompt as a fake user message. However, the Converse/ConverseStream API supports a top-level `system` field: `"system": [{ "text": "..." }]`. This is the correct approach — it preserves model behavior (system messages have different attention patterns than user messages in most models).

**Alternatives considered**:
- Synthetic user message (existing stub) → rejected (incorrect semantics, may degrade model quality)

## R6: New Dependencies

**Decision**: Add `aws-smithy-eventstream` and `aws-smithy-types` as workspace dependencies, gated behind the `bedrock` feature.

| Crate | Version | Purpose |
|---|---|---|
| `aws-smithy-eventstream` | 0.60.x | Binary frame decoding (`MessageFrameDecoder`) |
| `aws-smithy-types` | 1.x | Event-stream types (`Message`, `Header`, `HeaderValue`) |

**Rationale**: These are lightweight crates from the official AWS SDK that handle the binary event-stream protocol. They don't pull in the full AWS SDK dependency tree.

## R7: Comprehensive Model Catalog

**Decision**: Include all current Bedrock models across all provider families.

The catalog will cover 9+ provider families with ~50 models total:
- **Anthropic**: Claude Opus 4.6, Sonnet 4.6, Sonnet 4.5, Haiku 4.5, 3.7 Sonnet, 3.5 Sonnet v2, 3.5 Haiku, 3 Opus, 3 Haiku
- **Meta**: Llama 4 Scout, 4 Maverick, 3.3 70B, 3.2 (90B/11B/3B/1B), 3.1 (405B/70B/8B)
- **Amazon**: Nova 2 Pro, 2 Lite, Pro v1, Lite v1, Micro v1, Premier v1
- **Mistral**: Large 3, Large 2407, Pixtral Large, Ministral 3 (14B/8B/3B), Small, Mixtral 8x7B, 7B
- **DeepSeek**: R1, V3.2
- **AI21**: Jamba 1.5 Large, 1.5 Mini, Instruct
- **Cohere**: Command R+, Command R
- **OpenAI**: GPT-OSS 120B, 20B
- **Qwen**: Coder 480B, Coder 30B, 235B, 32B
- **Writer**: Palmyra X5, X4
- **Others**: Kimi K2.5, GLM 4.7, GLM 4.7 Flash, MiniMax M2.1

Model IDs use cross-region inference profile format (`us.` prefix) where that is the recommended access pattern.

## R8: Existing Stub Reuse

**Decision**: Retain and extend the existing `bedrock.rs` stub. Keep: struct definition, constructors, SigV4 signing, message conversion types, crypto helpers, Debug impl, Send+Sync assertion. Replace: `converse()` method with streaming `converse_stream()`, add event-stream parsing, add `system` field to request, update response types for streaming events.

**Rationale**: ~60% of the existing code is reusable (SigV4 signing, request types, message conversion, helpers). The main change is replacing the single-response `converse()` with a streaming `converse_stream()` that reads binary event-stream frames and maps them to `AssistantMessageEvent`s.
