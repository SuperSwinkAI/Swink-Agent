//! Convenience re-exports for common Swink-Agent types.
//!
//! ```
//! use swink_agent::prelude::*;
//! ```

pub use crate::{
    Agent, AgentContext, AgentError, AgentEvent, AgentHandle, AgentId, AgentMailbox, AgentMessage,
    AgentOptions, AgentOrchestrator, AgentRef, AgentRegistry, AgentResult, AgentStatus, AgentTool,
    AgentToolResult, AssistantMessage, AssistantMessageEvent, AsyncContextTransformer, Checkpoint,
    CheckpointStore, ContentBlock, ContextSummarizer, ContextTransformer, ContextVersion,
    ContextVersionMeta, ContextVersionStore, Cost, DefaultTokenCounter, Emission, EventForwarderFn,
    FnTool, InMemoryVersionStore, IntoTool, LlmMessage, LoopCheckpoint, MetricsCollector,
    ModelFallback, ModelSpec, StopReason, StreamErrorKind, StreamFn,
    StreamMiddleware, StreamOptions, SubAgent, TokenCounter,
    Usage, UserMessage, VersioningTransformer,
};
