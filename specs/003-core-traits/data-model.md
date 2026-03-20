# Data Model: Core Traits

**Feature**: 003-core-traits
**Date**: 2026-03-20

## Overview

Defines the three pluggable trait boundaries of the agent harness plus
their supporting types. No business logic — only trait definitions,
result types, validation, accumulation, and the default retry strategy.

## Entities

### AgentTool (trait)

Object-safe trait for tool implementations. Used as `Arc<dyn AgentTool>`.

| Method | Signature | Description |
|--------|-----------|-------------|
| name | `&self -> &str` | Unique identifier for routing |
| label | `&self -> &str` | Human-readable display name |
| description | `&self -> &str` | Natural-language description for LLM |
| parameters_schema | `&self -> Value` | JSON Schema for input validation |
| execute | `async &self, call_id, args, cancel_token, update_cb -> AgentToolResult` | Async execution |

### AgentToolResult

| Field | Type | Description |
|-------|------|-------------|
| content | Vec<ContentBlock> | Content returned to the LLM |
| details | Option<Value> | Structured data for logging/display |
| is_error | bool | Distinguishes success from failure |

Constructors: `text(s)` (success), `error(s)` (failure with is_error=true)

### StreamFn (trait)

Object-safe trait for LLM provider streaming. The sole provider boundary.

| Method | Signature | Description |
|--------|-----------|-------------|
| call | `async &self, model, context, options, cancel_token -> Stream<AssistantMessageEvent>` | Returns event stream |

### StreamOptions

| Field | Type | Description |
|-------|------|-------------|
| temperature | Option<f64> | Sampling temperature |
| max_tokens | Option<u32> | Output token limit |
| session_id | Option<String> | Provider-side session identifier |
| transport | Transport | Preferred protocol (default: SSE) |

### AssistantMessageEvent (enum)

| Variant | Payload | Description |
|---------|---------|-------------|
| Start | provider, model | Stream begins |
| TextStart | index | Text block begins |
| TextDelta | index, text | Text fragment |
| TextEnd | index | Text block ends |
| ThinkingStart | index | Thinking block begins |
| ThinkingDelta | index, thinking | Thinking fragment |
| ThinkingEnd | index | Thinking block ends |
| ToolCallStart | index, id, name | Tool call begins |
| ToolCallDelta | index, json_fragment | Partial JSON argument |
| ToolCallEnd | index | Tool call ends (partial JSON consumed) |
| Done | usage, cost, stop_reason | Terminal: success |
| Error | error_message | Terminal: error |

### AssistantMessageDelta (enum)

| Variant | Fields | Description |
|---------|--------|-------------|
| TextDelta | index, text | Appended text fragment |
| ThinkingDelta | index, thinking | Appended reasoning fragment |
| ToolCallDelta | index, json_fragment | Appended JSON argument fragment |

### RetryStrategy (trait)

| Method | Signature | Description |
|--------|-----------|-------------|
| should_retry | `&self, error: &AgentError, attempt: usize -> bool` | Retry decision |
| delay | `&self, attempt: usize -> Duration` | Delay before next attempt |

### DefaultRetryStrategy

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| max_attempts | usize | 3 | Maximum retry attempts |
| base_delay | Duration | 1s | Initial delay |
| max_delay | Duration | 60s | Delay cap |
| multiplier | f64 | 2.0 | Exponential multiplier |
| jitter | bool | true | Enable jitter [0.5, 1.5) range |

Retries only: `ModelThrottled`, `NetworkError`. All other errors are non-retryable.

## Validation Rules

- Tool arguments validated against `parameters_schema()` via `jsonschema` before `execute()`
- Empty arguments `{}` are valid — schema decides correctness
- Empty partial JSON string on ToolCallEnd → `{}` (not null)
- Out-of-order streaming events → error + terminate
- Empty stream (no events) → stream error
- Retry delay clamped to `max_delay`
