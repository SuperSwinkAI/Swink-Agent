# Public API Contract: Foundation Types & Errors

**Feature**: 002-foundation-types-errors
**Date**: 2026-03-20

## Re-exports from `lib.rs`

All types below MUST be re-exported from `src/lib.rs` so consumers access
them via `use swink_agent::*` without reaching into submodules.

## ContentBlock API

```rust
pub enum ContentBlock {
    Text { text: String },
    Thinking { thinking: String, signature: Option<String> },
    ToolCall { id: String, name: String, arguments: serde_json::Value, partial_json: Option<String> },
    Image { source: ImageSource },
}

pub enum ImageSource {
    Base64 { data: String, media_type: String },
    Url { url: String, media_type: String },
    File { path: PathBuf, media_type: String },
}
```

## LlmMessage API

```rust
pub enum LlmMessage {
    User { content: Vec<ContentBlock>, timestamp: SystemTime },
    Assistant {
        content: Vec<ContentBlock>,
        provider: String,
        model: String,
        usage: Usage,
        cost: Cost,
        stop_reason: StopReason,
        error_message: Option<String>,
        timestamp: SystemTime,
    },
    ToolResult {
        tool_call_id: String,
        content: Vec<ContentBlock>,
        is_error: bool,
        timestamp: SystemTime,
    },
}
```

## AgentMessage API

```rust
pub enum AgentMessage {
    Llm(LlmMessage),
    Custom(Box<dyn CustomMessage>),
}

impl AgentMessage {
    pub fn downcast_ref<T: CustomMessage>(&self) -> Result<&T, DowncastError>;
}

pub trait CustomMessage: Send + Sync + Any + 'static {
    fn as_any(&self) -> &dyn Any;
    fn type_name(&self) -> &str;
}
```

## Usage & Cost API

```rust
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub total_tokens: u64,
}
// Implements: Default, Add, AddAssign, Serialize, Deserialize

pub struct Cost {
    pub input_cost: f64,
    pub output_cost: f64,
    pub cache_read_cost: f64,
    pub cache_write_cost: f64,
    pub total_cost: f64,
}
// Implements: Default, Add, AddAssign, Serialize, Deserialize
```

## StopReason & ModelSpec API

```rust
pub enum StopReason { Stop, Length, ToolUse, Aborted, Error }

pub enum ThinkingLevel { Off, Minimal, Low, Medium, High, ExtraHigh }

pub struct ModelSpec {
    pub provider: String,
    pub model_id: String,
    pub thinking_level: ThinkingLevel,
    pub thinking_budgets: Option<HashMap<ThinkingLevel, u32>>,
}
```

## AgentResult API

```rust
pub struct AgentResult {
    pub messages: Vec<AgentMessage>,
    pub stop_reason: StopReason,
    pub usage: Usage,
    pub error: Option<String>,
}
```

## AgentContext API

```rust
pub struct AgentContext {
    pub system_prompt: String,
    pub messages: Vec<AgentMessage>,
    pub tools: Vec<Arc<dyn AgentTool>>,  // AgentTool defined in feature 003
}
```

## AgentError API

```rust
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("Context window overflow for model {model}")]
    ContextWindowOverflow { model: String },
    #[error("Model rate limited")]
    ModelThrottled,
    #[error("Network error")]
    NetworkError,
    #[error("Structured output failed after {attempts} attempts: {last_error}")]
    StructuredOutputFailed { attempts: usize, last_error: String },
    #[error("Agent is already running")]
    AlreadyRunning,
    #[error("No messages in context")]
    NoMessages,
    #[error("Cannot continue: last message is an assistant message")]
    InvalidContinue,
    #[error("Stream error: {source}")]
    StreamError { source: Box<dyn Error + Send + Sync> },
    #[error("Agent run was aborted")]
    Aborted,
}

#[derive(Debug, thiserror::Error)]
#[error("Downcast failed: expected {expected}, got {actual}")]
pub struct DowncastError {
    pub expected: &'static str,
    pub actual: String,
}
```

## Derive Requirements

All concrete types (not trait objects) MUST derive:
- `Debug`, `Clone` (where possible)
- `Serialize`, `Deserialize` (via serde)

Enum types MUST additionally derive:
- `PartialEq`, `Eq` (for StopReason, ThinkingLevel)

Thread-safety: all public types MUST be `Send + Sync`.
