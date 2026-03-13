#![forbid(unsafe_code)]
mod agent;
mod context;
mod error;
mod fn_tool;
mod loop_;
pub mod message_provider;
mod proxy;
mod retry;
mod schema;
pub mod stream;
pub mod tool;
mod tool_middleware;
pub mod tools;
pub mod types;
mod util;

// Re-exports
pub use agent::{
    Agent, AgentOptions, AgentState, FollowUpMode, SteeringMode, SubscriptionId, default_convert,
};
pub use context::sliding_window;
pub use error::AgentError;
pub use fn_tool::FnTool;
pub use loop_::{AgentEvent, AgentLoopConfig, agent_loop, agent_loop_continue};
pub use message_provider::{MessageProvider, from_fns};
pub use proxy::ProxyStreamFn;
pub use retry::{DefaultRetryStrategy, RetryStrategy};
pub use schema::schema_for;
pub use schemars::JsonSchema;
pub use stream::{
    AssistantMessageDelta, AssistantMessageEvent, StreamFn, StreamOptions, StreamTransport,
    accumulate_message,
};
pub use tool::{
    AgentTool, AgentToolResult, ApprovalMode, ToolApproval, ToolApprovalRequest,
    redact_sensitive_values, selective_approve, unknown_tool_result, validate_schema,
    validate_tool_arguments, validation_error_result,
};
pub use tool_middleware::ToolMiddleware;
pub use tools::{BashTool, ReadFileTool, WriteFileTool};
pub use types::{
    AgentContext, AgentMessage, AgentResult, AssistantMessage, ContentBlock, Cost, CustomMessage,
    ImageSource, LlmMessage, ModelSpec, StopReason, ThinkingBudgets, ThinkingLevel,
    ToolResultMessage, Usage, UserMessage,
};
