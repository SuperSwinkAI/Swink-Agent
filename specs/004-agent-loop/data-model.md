# Data Model: Agent Loop

**Feature**: 004-agent-loop
**Date**: 2026-03-20

## Overview

The agent loop is stateless — it takes configuration and context as
input and returns an event stream. Internal state is tracked via
`LoopState` which lives for the duration of a single loop invocation.

## Entities

### AgentLoopConfig

Configuration passed to the loop entry points. Struct.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| model | ModelSpec | Yes | Model specification for the provider |
| stream_options | StreamOptions | Yes | Per-call options (temperature, max tokens, etc.) |
| retry_strategy | Box<dyn RetryStrategy> | Yes | Retry logic for transient failures |
| convert_to_llm | ConvertToLlmFn | Yes | `Fn(&AgentMessage) → Option<LlmMessage>` — filters/converts messages for provider |
| transform_context | Option<TransformContextFn> | No | Synchronous context pruning/budget hook. Identity if absent. |
| get_api_key | Option<GetApiKeyFn> | No | Async callback for dynamic API key resolution |
| get_steering_messages | Option<GetSteeringMessagesFn> | No | Async callback polled after each tool completion |
| get_follow_up_messages | Option<GetFollowUpMessagesFn> | No | Async callback polled when agent would stop |

### LoopState

Internal mutable state for a single loop invocation.

| Field | Type | Description |
|-------|------|-------------|
| context_messages | Vec<AgentMessage> | Current message history |
| overflow_signal | bool | Set on context overflow, reset after transform_context |
| pending_messages | Vec<AgentMessage> | Steering/follow-up messages queued for next turn |
| system_prompt | String | System prompt text |
| tools | Vec<Arc<dyn AgentTool>> | Available tools |

### AgentEvent

Lifecycle event enum. All variants listed.

| Category | Variant | Payload | Description |
|----------|---------|---------|-------------|
| Agent | AgentStart | — | Loop begins |
| Agent | AgentEnd | `messages: Vec<AgentMessage>` | Loop exits, carries all produced messages |
| Turn | TurnStart | — | Turn begins |
| Turn | TurnEnd | `message: AssistantMessage, tool_results: Vec<ToolResultMessage>, reason: TurnEndReason` | Turn ends |
| Message | MessageStart | — | Message streaming begins |
| Message | MessageUpdate | `delta: AssistantMessageDelta` | Incremental content delta |
| Message | MessageEnd | `message: AssistantMessage` | Message finalized |
| Tool | ToolExecutionStart | `call_id: String, name: String, arguments: Value` | Tool begins |
| Tool | ToolExecutionUpdate | `call_id: String, update: String` | Intermediate tool output |
| Tool | ToolExecutionEnd | `call_id: String, result: AgentToolResult, is_error: bool` | Tool completes |
| Context | ContextCompacted | `report: String` | Context was compacted by transform hook |

### TurnEndReason

Why the turn ended. Enum.

| Variant | Description |
|---------|-------------|
| Complete | Natural end — no tool calls |
| ToolsExecuted | Tool calls processed, continuing |
| SteeringInterrupt | Steering message interrupted tools |
| Error | Error during turn |
| Aborted | Cancellation token triggered |

## State Transitions

### Loop Lifecycle

```text
                     ┌─────────────────────────────────────────┐
                     │              OUTER LOOP                  │
                     │                                          │
   Entry ──► AgentStart ──► ┌─────────────────────────┐        │
                            │       INNER LOOP         │        │
                            │                          │        │
                            │  TurnStart               │        │
                            │    ├─ transform_context   │        │
                            │    ├─ convert_to_llm      │        │
                            │    ├─ get_api_key          │        │
                            │    ├─ call StreamFn        │        │
                            │    ├─ accumulate message   │        │
                            │    │                       │        │
                            │    ├─ [no tools] ──► break │        │
                            │    │                       │        │
                            │    ├─ [tools] ──►          │        │
                            │    │  ├─ spawn per tool    │        │
                            │    │  ├─ poll steering     │        │
                            │    │  └─ collect results   │        │
                            │    │                       │        │
                            │  TurnEnd                   │        │
                            │    ├─ [steering] ──► loop  │        │
                            │    └─ [no steering] ──►    │        │
                            │       poll steering again  │        │
                            │       └─ loop or break     │        │
                            └────────────┬───────────────┘        │
                                         │                        │
                            [error/abort] ──► AgentEnd ──► Exit   │
                            [normal exit] ──► poll follow-up      │
                              ├─ [follow-up] ──► re-enter inner   │
                              └─ [none] ──► AgentEnd ──► Exit     │
                     └────────────────────────────────────────────┘
```

## Validation Rules

- Events MUST be emitted in lifecycle order (FR-003)
- Tool calls MUST execute concurrently (FR-007)
- Overflow signal MUST reset after transform_context (CLAUDE.md lesson)
- Error/abort MUST skip follow-up polling (FR-011)
- Transform MUST run before convert-to-llm (FR-005)
