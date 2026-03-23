#![forbid(unsafe_code)]
mod agent;
pub mod agent_options;
pub(crate) mod agent_subscriptions;
mod async_context_transformer;
mod budget_guard;
mod checkpoint;
mod config;
mod context;
mod context_transformer;
mod context_version;
pub mod convert;
pub mod display;
mod emit;
mod error;
mod event_forwarder;
mod fallback;
mod fn_tool;
mod handle;
mod loop_;
mod loop_policy;
pub mod message_provider;
mod messaging;
pub mod metrics;
mod model_catalog;
mod model_presets;
mod orchestrator;
mod post_turn_hook;
mod registry;
mod retry;
mod schema;
pub mod stream;
mod stream_middleware;
mod sub_agent;
pub mod tool;
mod tool_call_transformer;
mod tool_execution_policy;
mod tool_middleware;
mod tool_validator;
pub mod tools;
pub mod types;
mod util;

#[cfg(feature = "test-helpers")]
pub mod testing;

pub mod prelude;

// Re-exports
pub use agent::{
    Agent, AgentOptions, AgentState, DEFAULT_PLAN_MODE_ADDENDUM, FollowUpMode, SteeringMode,
    SubscriptionId, default_convert,
};
pub use async_context_transformer::AsyncContextTransformer;
pub use budget_guard::{BudgetExceeded, BudgetGuard};
pub use checkpoint::{Checkpoint, CheckpointStore, LoopCheckpoint};
pub use config::{
    AgentConfig, ApprovalModeConfig, BudgetGuardConfig, FollowUpModeConfig, RetryConfig,
    SteeringModeConfig, StreamOptionsConfig,
};
pub use context::{DefaultTokenCounter, TokenCounter, estimate_tokens, sliding_window};
pub use context_transformer::{CompactionReport, ContextTransformer, SlidingWindowTransformer};
pub use context_version::{
    ContextSummarizer, ContextVersion, ContextVersionMeta, ContextVersionStore,
    InMemoryVersionStore, VersioningTransformer,
};
pub use convert::{MessageConverter, ToolSchema, convert_messages, extract_tool_schemas};
pub use emit::Emission;
pub use error::{AgentError, DowncastError};
pub use event_forwarder::EventForwarderFn;
pub use fallback::ModelFallback;
pub use fn_tool::FnTool;
pub use handle::{AgentHandle, AgentStatus};
pub use loop_::{AgentEvent, AgentLoopConfig, TurnEndReason, agent_loop, agent_loop_continue};
pub use loop_policy::{ComposedPolicy, CostCapPolicy, LoopPolicy, MaxTurnsPolicy, PolicyContext};
pub use message_provider::{
    ChannelMessageProvider, ComposedMessageProvider, MessageProvider, MessageSender, from_fns,
    message_channel,
};
pub use messaging::{AgentMailbox, send_to};
pub use metrics::{MetricsCollector, ToolExecMetrics, TurnMetrics};
pub use model_catalog::{
    ApiVersion, AuthMode, CatalogPreset, ModelCatalog, PresetCapability, PresetCatalog,
    PresetStatus, ProviderCatalog, ProviderKind, model_catalog,
};
pub use model_presets::{ModelConnection, ModelConnections};
pub use orchestrator::{
    AgentOrchestrator, AgentRequest, DefaultSupervisor, OrchestratedHandle, SupervisorAction,
    SupervisorPolicy,
};
pub use post_turn_hook::{PostTurnAction, PostTurnContext, PostTurnHook};
pub use registry::{AgentId, AgentRef, AgentRegistry};
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
    AgentTool, AgentToolResult, ApprovalMode, ToolApproval, ToolApprovalRequest, ToolMetadata,
    redact_sensitive_values, selective_approve, unknown_tool_result, validate_schema,
    validate_tool_arguments, validation_error_result,
};
pub use tool_call_transformer::ToolCallTransformer;
pub use tool_execution_policy::{
    PriorityFn, ToolCallSummary, ToolExecutionPolicy, ToolExecutionStrategy,
};
pub use tool_middleware::ToolMiddleware;
pub use tool_validator::ToolValidator;
#[cfg(feature = "builtin-tools")]
pub use tools::{BashTool, ReadFileTool, WriteFileTool, builtin_tools};
pub use types::{
    AgentContext, AgentMessage, AgentResult, AssistantMessage, ContentBlock, Cost, CustomMessage,
    CustomMessageRegistry, ImageSource, LlmMessage, ModelCapabilities, ModelSpec, StopReason,
    ThinkingBudgets, ThinkingLevel, ToolResultMessage, TurnSnapshot, Usage, UserMessage,
    deserialize_custom_message, serialize_custom_message,
};
pub use util::now_timestamp;

pub use display::{CoreDisplayMessage, DisplayRole, IntoDisplayMessages};
