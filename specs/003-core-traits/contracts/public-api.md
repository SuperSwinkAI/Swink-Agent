# Public API Contract: Core Traits

**Feature**: 003-core-traits
**Date**: 2026-03-20

## AgentTool API

```rust
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &str;
    fn label(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;
    async fn execute(
        &self,
        call_id: &str,
        arguments: serde_json::Value,
        cancellation_token: CancellationToken,
        update_callback: Option<Box<dyn Fn(String) + Send>>,
    ) -> AgentToolResult;
}

pub struct AgentToolResult {
    pub content: Vec<ContentBlock>,
    pub details: Option<serde_json::Value>,
    pub is_error: bool,
}

impl AgentToolResult {
    pub fn text(text: impl Into<String>) -> Self;
    pub fn error(text: impl Into<String>) -> Self;
}

pub fn validate_tool_arguments(
    schema: &serde_json::Value,
    arguments: &serde_json::Value,
) -> Result<(), Vec<String>>;
```

## StreamFn API

```rust
pub trait StreamFn: Send + Sync {
    async fn call(
        &self,
        model: &ModelSpec,
        context: &AgentContext,
        options: &StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send>>;
}

pub struct StreamOptions {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub session_id: Option<String>,
    pub transport: Transport,
}

pub enum AssistantMessageEvent {
    Start { provider: String, model: String },
    TextStart { index: usize },
    TextDelta { index: usize, text: String },
    TextEnd { index: usize },
    ThinkingStart { index: usize },
    ThinkingDelta { index: usize, thinking: String },
    ThinkingEnd { index: usize },
    ToolCallStart { index: usize, id: String, name: String },
    ToolCallDelta { index: usize, json_fragment: String },
    ToolCallEnd { index: usize },
    Done { usage: Usage, cost: Cost, stop_reason: StopReason },
    Error { error_message: String },
}

pub enum AssistantMessageDelta {
    TextDelta { index: usize, text: String },
    ThinkingDelta { index: usize, thinking: String },
    ToolCallDelta { index: usize, json_fragment: String },
}

/// Consume an event stream and produce a finalized AssistantMessage.
pub async fn accumulate_message(
    stream: impl Stream<Item = AssistantMessageEvent>,
) -> Result<AssistantMessage, String>;
```

## RetryStrategy API

```rust
pub trait RetryStrategy: Send + Sync {
    fn should_retry(&self, error: &AgentError, attempt: usize) -> bool;
    fn delay(&self, attempt: usize) -> Duration;
}

pub struct DefaultRetryStrategy {
    pub max_attempts: usize,
    pub base_delay: Duration,
    pub max_delay: Duration,
    pub multiplier: f64,
    pub jitter: bool,
}

impl Default for DefaultRetryStrategy {
    // max_attempts: 3, base_delay: 1s, max_delay: 60s, multiplier: 2.0, jitter: true
}
```

## Behavioral Contracts

- `validate_tool_arguments` MUST be called before `execute`; invalid args never reach execute
- `accumulate_message` enforces strict event ordering; violations return error
- Empty partial JSON on ToolCallEnd → `{}` (empty object, not null)
- `DefaultRetryStrategy` retries ONLY `ModelThrottled` and `NetworkError`
- Jitter multiplies delay by a factor in [0.5, 1.5)
- Delay is clamped to `max_delay` before jitter is applied
