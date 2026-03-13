//! Convenience re-exports for common Swink-Agent types.
//!
//! ```
//! use swink_agent::prelude::*;
//! ```

pub use crate::{
    Agent, AgentContext, AgentError, AgentEvent, AgentMessage, AgentOptions, AgentResult,
    AgentTool, AgentToolResult, AssistantMessage, AssistantMessageEvent, ContentBlock,
    ContextTransformer, Cost, FnTool, LlmMessage, LoopPolicy, ModelSpec, StopReason,
    StreamErrorKind, StreamFn, StreamMiddleware, StreamOptions, SubAgent, ToolCallTransformer,
    ToolValidator, Usage, UserMessage,
};
