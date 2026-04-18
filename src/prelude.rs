//! Convenience re-exports for common Swink-Agent types.
//!
//! ```
//! use std::sync::Arc;
//! use swink_agent::prelude::*;
//!
//! let tool = Arc::new(FnTool::new("echo", "Echo", "Echoes input"));
//! let _logged = ToolMiddleware::with_logging(tool, |_name, _id, _is_start| {});
//! // ApprovalMode is available via the prelude without a separate import
//! let _mode: ApprovalMode = ApprovalMode::default();
//! ```

pub use crate::{
    Agent, AgentContext, AgentError, AgentEvent, AgentHandle, AgentId, AgentMailbox, AgentMessage,
    AgentOptions, AgentOrchestrator, AgentRef, AgentRegistry, AgentResult, AgentStatus, AgentTool,
    AgentToolResult, ApprovalMode, AssistantMessage, AssistantMessageEvent,
    AsyncContextTransformer, Checkpoint, CheckpointStore, ContentBlock, ContextSummarizer,
    ContextTransformer, ContextVersion, ContextVersionMeta, ContextVersionStore, Cost,
    DefaultTokenCounter, Emission, EventForwarderFn, FnTool, InMemoryVersionStore, IntoTool,
    LlmMessage, LoopCheckpoint, MetricsCollector, ModelConnection, ModelConnections,
    ModelConnectionsBuilder, ModelFallback, ModelSpec, StopReason, StreamErrorKind, StreamFn,
    StreamMiddleware, StreamOptions, SubAgent, TokenCounter, ToolMiddleware, Usage, UserMessage,
    VersioningTransformer,
};

#[cfg(feature = "builtin-tools")]
pub use crate::{BashTool, ReadFileTool, WriteFileTool, builtin_tools};
