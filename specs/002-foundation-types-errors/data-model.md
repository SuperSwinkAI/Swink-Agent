# Data Model: Foundation Types & Errors

**Feature**: 002-foundation-types-errors
**Date**: 2026-03-20

## Overview

Defines the core data types that all other modules depend on. These types
represent conversation messages, content blocks, usage tracking, error
conditions, and model configuration. No business logic — only data
definitions, constructors, and trait implementations.

## Entities

### ContentBlock

Atomic unit of message content. Enum with four variants.

| Variant | Fields | Description |
|---------|--------|-------------|
| Text | `text: String` | Plain text content |
| Thinking | `thinking: String`, `signature: Option<String>` | Reasoning trace with optional provider verification signature |
| ToolCall | `id: String`, `name: String`, `arguments: serde_json::Value`, `partial_json: Option<String>` | Tool invocation with parsed args and optional streaming buffer |
| Image | `source: ImageSource` | Image data from a supported source |

### ImageSource

Enum with three source type variants.

| Variant | Fields | Description |
|---------|--------|-------------|
| Base64 | `data: String`, `media_type: String` | Inline base64-encoded image data |
| Url | `url: String`, `media_type: String` | URL reference to image |
| File | `path: PathBuf`, `media_type: String` | Local file path to image |

### LlmMessage

Standard conversation message. Enum with three variants.

| Variant | Fields | Description |
|---------|--------|-------------|
| User | `content: Vec<ContentBlock>`, `timestamp: SystemTime` | User input (text and optional images) |
| Assistant | `content: Vec<ContentBlock>`, `provider: String`, `model: String`, `usage: Usage`, `cost: Cost`, `stop_reason: StopReason`, `error_message: Option<String>`, `timestamp: SystemTime` | LLM response with metadata |
| ToolResult | `tool_call_id: String`, `content: Vec<ContentBlock>`, `is_error: bool`, `timestamp: SystemTime` | Tool execution result |

### AgentMessage

Open wrapper that holds either an LlmMessage or a custom application-defined message.

| Variant | Fields | Description |
|---------|--------|-------------|
| Llm | `message: LlmMessage` | Standard conversation message |
| Custom | `message: Box<dyn CustomMessage>` | Application-defined extension |

### CustomMessage (trait)

Trait bounds: `Send + Sync + Any + 'static`

Required methods: `as_any(&self) -> &dyn Any`, `type_name(&self) -> &str`

Downcast method on AgentMessage: `downcast_ref<T>() -> Result<&T, DowncastError>`

### Usage

Token consumption counters. All fields `u64`. Default is all zeros (valid).

| Field | Type | Description |
|-------|------|-------------|
| input_tokens | u64 | Tokens in the prompt |
| output_tokens | u64 | Tokens generated |
| cache_read_tokens | u64 | Tokens read from cache |
| cache_write_tokens | u64 | Tokens written to cache |
| total_tokens | u64 | Total tokens consumed |

Implements `Add` and `AddAssign` for aggregation.

### Cost

Financial cost breakdown. All fields `f64`. Default is all zeros.

| Field | Type | Description |
|-------|------|-------------|
| input_cost | f64 | Cost for input tokens |
| output_cost | f64 | Cost for output tokens |
| cache_read_cost | f64 | Cost for cached reads |
| cache_write_cost | f64 | Cost for cache writes |
| total_cost | f64 | Total cost |

Implements `Add` and `AddAssign` for aggregation.

### StopReason

Why the LLM stopped generating. Enum.

| Variant | Description |
|---------|-------------|
| Stop | Natural end of generation |
| Length | Output token limit reached |
| ToolUse | Tool call requested |
| Aborted | Cancelled by caller |
| Error | Error during generation |

### ModelSpec

Target model configuration. Struct.

| Field | Type | Description |
|-------|------|-------------|
| provider | String | Backend identifier (e.g., "anthropic") |
| model_id | String | Model identifier (e.g., "claude-sonnet-4-6") |
| thinking_level | ThinkingLevel | Reasoning depth |
| thinking_budgets | Option<HashMap<ThinkingLevel, u32>> | Per-level token budget overrides |

### ThinkingLevel

Reasoning depth. Enum.

| Variant | Description |
|---------|-------------|
| Off | No reasoning |
| Minimal | Minimal reasoning |
| Low | Low reasoning |
| Medium | Medium reasoning |
| High | High reasoning |
| ExtraHigh | Maximum reasoning |

### AgentResult

Outcome of a complete agent run. Struct.

| Field | Type | Description |
|-------|------|-------------|
| messages | Vec<AgentMessage> | All messages produced during the run |
| stop_reason | StopReason | Final stop reason |
| usage | Usage | Aggregated token usage across all turns |
| error | Option<String> | Error string if run ended in error |

### AgentContext

Immutable snapshot passed into each loop turn. Struct.

| Field | Type | Description |
|-------|------|-------------|
| system_prompt | String | System prompt text |
| messages | Vec<AgentMessage> | Current message history |
| tools | Vec<Arc<dyn AgentTool>> | Available tools (AgentTool defined in feature 003) |

### AgentError

Typed error taxonomy. Enum derived with `thiserror`.

| Variant | Fields | Display Message |
|---------|--------|----------------|
| ContextWindowOverflow | `model: String` | "Context window overflow for model {model}" |
| ModelThrottled | (none) | "Model rate limited" |
| NetworkError | (none) | "Network error" |
| StructuredOutputFailed | `attempts: usize`, `last_error: String` | "Structured output failed after {attempts} attempts: {last_error}" |
| AlreadyRunning | (none) | "Agent is already running" |
| NoMessages | (none) | "No messages in context" |
| InvalidContinue | (none) | "Cannot continue: last message is an assistant message" |
| StreamError | `source: Box<dyn Error + Send + Sync>` | "Stream error: {source}" |
| Aborted | (none) | "Agent run was aborted" |

### DowncastError

Error returned when custom message downcast fails. Struct.

| Field | Type | Description |
|-------|------|-------------|
| expected | &'static str | Expected type name |
| actual | String | Actual type name from `CustomMessage::type_name()` |

## Validation Rules

- All public types MUST be `Send + Sync` (compile-time verified)
- All types with data MUST derive `Serialize` + `Deserialize` (except trait objects)
- `AgentError` MUST implement `std::error::Error` via thiserror
- `Usage` and `Cost` aggregation MUST be arithmetically correct
- `ContentBlock::ToolCall` with `arguments: {}` MUST be accepted as valid

## Relationships

- `LlmMessage` contains `Vec<ContentBlock>`, `Usage`, `Cost`, `StopReason`
- `AgentMessage` wraps `LlmMessage` or `Box<dyn CustomMessage>`
- `AgentResult` contains `Vec<AgentMessage>`, `StopReason`, `Usage`
- `AgentContext` contains `Vec<AgentMessage>` and tool references
- `ModelSpec` contains `ThinkingLevel`
