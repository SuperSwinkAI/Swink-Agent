# Public API Contract: Agent Loop

**Feature**: 004-agent-loop
**Date**: 2026-03-20

## Loop Entry Points

```rust
/// Start a new loop with prompt messages prepended to context.
pub fn agent_loop(
    prompt_messages: Vec<AgentMessage>,
    context: AgentContext,
    config: AgentLoopConfig,
    cancellation_token: CancellationToken,
) -> impl Stream<Item = AgentEvent>;

/// Resume from existing context without adding new messages.
pub fn agent_loop_continue(
    context: AgentContext,
    config: AgentLoopConfig,
    cancellation_token: CancellationToken,
) -> impl Stream<Item = AgentEvent>;
```

## AgentLoopConfig

```rust
pub struct AgentLoopConfig {
    pub model: ModelSpec,
    pub stream_options: StreamOptions,
    pub retry_strategy: Box<dyn RetryStrategy>,
    pub convert_to_llm: ConvertToLlmFn,
    pub transform_context: Option<TransformContextFn>,
    pub get_api_key: Option<GetApiKeyFn>,
    pub get_steering_messages: Option<GetSteeringMessagesFn>,
    pub get_follow_up_messages: Option<GetFollowUpMessagesFn>,
}
```

## Callback Type Aliases

```rust
/// Converts an AgentMessage to an optional LlmMessage.
/// Returns None to filter the message out (e.g., custom messages).
pub type ConvertToLlmFn = Arc<dyn Fn(&AgentMessage) -> Option<LlmMessage> + Send + Sync>;

/// Synchronous context transformation hook.
/// Receives mutable message history and overflow signal.
/// Returns an optional compaction report string.
pub type TransformContextFn = Arc<dyn Fn(&mut Vec<AgentMessage>, bool) -> Option<String> + Send + Sync>;

/// Async API key resolution.
pub type GetApiKeyFn = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Option<String>> + Send>> + Send + Sync>;

/// Async steering message poll.
pub type GetSteeringMessagesFn = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Vec<AgentMessage>> + Send>> + Send + Sync>;

/// Async follow-up message poll.
pub type GetFollowUpMessagesFn = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Vec<AgentMessage>> + Send>> + Send + Sync>;
```

## AgentEvent

```rust
pub enum AgentEvent {
    AgentStart,
    AgentEnd { messages: Vec<AgentMessage> },
    TurnStart,
    TurnEnd {
        message: AssistantMessage,
        tool_results: Vec<ToolResultMessage>,
        reason: TurnEndReason,
    },
    MessageStart,
    MessageUpdate { delta: AssistantMessageDelta },
    MessageEnd { message: AssistantMessage },
    ToolExecutionStart { call_id: String, name: String, arguments: Value },
    ToolExecutionUpdate { call_id: String, update: String },
    ToolExecutionEnd { call_id: String, result: AgentToolResult, is_error: bool },
    ContextCompacted { report: String },
}

pub enum TurnEndReason {
    Complete,
    ToolsExecuted,
    SteeringInterrupt,
    Error,
    Aborted,
}
```

## Behavioral Contracts

- `agent_loop` prepends `prompt_messages` to `context.messages` before the first turn
- `agent_loop_continue` validates that the last message is not an assistant message (returns `AgentError::InvalidContinue`)
- Both return `AgentError::NoMessages` if context has no messages
- `transform_context` receives `overflow_signal = true` after a context overflow error; the signal resets after the call
- When `transform_context` is `None`, context passes through unchanged (identity behavior)
- Steering messages with no active tools are queued as pending for the next turn
- Error/abort exits skip `get_follow_up_messages` — `AgentEnd` is emitted immediately
