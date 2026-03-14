//! Convenience re-exports for common Swink-Agent types.
//!
//! ```
//! use swink_agent::prelude::*;
//! ```

pub use crate::{
    Agent, AgentContext, AgentError, AgentEvent, AgentHandle, AgentId, AgentMailbox, AgentMessage,
    AgentOptions, AgentRef, AgentRegistry, AgentResult, AgentStatus, AgentTool, AgentToolResult,
    AssistantMessage, AssistantMessageEvent, ContentBlock, ContextTransformer, Cost, Emission,
    EventForwarderFn, FnTool, LlmMessage, LoopPolicy, ModelSpec, StopReason, StreamErrorKind,
    StreamFn, StreamMiddleware, StreamOptions, SubAgent, ToolCallTransformer, ToolValidator, Usage,
    UserMessage,
};
