#![forbid(unsafe_code)]
pub mod agent;
pub mod context;
pub mod error;
pub mod loop_;
pub mod proxy;
pub mod retry;
pub mod stream;
pub mod tool;
pub mod tools;
pub mod types;

// Re-exports
pub use agent::{Agent, AgentOptions, AgentState, FollowUpMode, SteeringMode, SubscriptionId};
pub use context::sliding_window;
pub use error::HarnessError;
pub use loop_::{AgentEvent, AgentLoopConfig, agent_loop, agent_loop_continue};
pub use proxy::ProxyStreamFn;
pub use retry::{DefaultRetryStrategy, RetryStrategy};
pub use stream::{
    AssistantMessageDelta, AssistantMessageEvent, StreamFn, StreamOptions, StreamTransport,
    accumulate_message,
};
pub use tool::{
    AgentTool, AgentToolResult, ApprovalMode, ToolApproval, ToolApprovalRequest,
    selective_approve, unknown_tool_result, validate_tool_arguments, validation_error_result,
};
pub use tools::{BashTool, ReadFileTool, WriteFileTool};
pub use types::{
    AgentContext, AgentMessage, AgentResult, AssistantMessage, ContentBlock, Cost, CustomMessage,
    ImageSource, LlmMessage, ModelSpec, StopReason, ThinkingBudgets, ThinkingLevel,
    ToolResultMessage, Usage, UserMessage,
};
