#![forbid(unsafe_code)]
mod agent;
mod context;
mod context_transformer;
mod error;
mod fn_tool;
mod loop_;
mod loop_policy;
pub mod message_provider;
mod retry;
mod schema;
pub mod stream;
mod stream_middleware;
mod sub_agent;
pub mod tool;
mod tool_call_transformer;
mod tool_middleware;
mod tool_validator;
pub mod tools;
pub mod types;
mod util;

pub mod prelude;

// Re-exports
pub use agent::{
    Agent, AgentOptions, AgentState, FollowUpMode, SteeringMode, SubscriptionId, default_convert,
};
pub use context::sliding_window;
pub use context_transformer::{CompactionReport, ContextTransformer, SlidingWindowTransformer};
pub use error::AgentError;
pub use fn_tool::FnTool;
pub use loop_::{AgentEvent, AgentLoopConfig, TurnEndReason, agent_loop, agent_loop_continue};
pub use loop_policy::{ComposedPolicy, CostCapPolicy, LoopPolicy, MaxTurnsPolicy, PolicyContext};
pub use message_provider::{MessageProvider, from_fns};
pub use retry::{DefaultRetryStrategy, RetryStrategy};
pub use schema::schema_for;
pub use schemars::JsonSchema;
pub use stream::{
    AssistantMessageDelta, AssistantMessageEvent, StreamErrorKind, StreamFn, StreamOptions,
    StreamTransport, accumulate_message,
};
pub use stream_middleware::StreamMiddleware;
pub use sub_agent::SubAgent;
pub use tool::{
    AgentTool, AgentToolResult, ApprovalMode, ToolApproval, ToolApprovalRequest,
    redact_sensitive_values, selective_approve, unknown_tool_result, validate_schema,
    validate_tool_arguments, validation_error_result,
};
pub use tool_call_transformer::ToolCallTransformer;
pub use tool_middleware::ToolMiddleware;
pub use tool_validator::ToolValidator;
#[cfg(feature = "builtin-tools")]
pub use tools::{BashTool, ReadFileTool, WriteFileTool};
pub use types::{
    AgentContext, AgentMessage, AgentResult, AssistantMessage, ContentBlock, Cost, CustomMessage,
    ImageSource, LlmMessage, ModelSpec, StopReason, ThinkingBudgets, ThinkingLevel,
    ToolResultMessage, Usage, UserMessage,
};
pub use util::now_timestamp;
