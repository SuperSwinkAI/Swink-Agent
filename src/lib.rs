#![forbid(unsafe_code)]
//! # Encapsulation regression
//!
//! The `credential` module is private; consumers must use the canonical
//! re-exports at the crate root rather than reaching into submodule paths.
//!
//! ```compile_fail
//! use swink_agent::credential::ResolvedCredential;
//! ```
mod agent;
mod agent_id;
pub(crate) mod agent_options;
pub(crate) mod agent_subscriptions;
#[cfg(feature = "artifact-store")]
pub mod artifact;
mod async_context_transformer;
pub(crate) mod block_accumulator;
mod checkpoint;
mod config;
mod context;
pub(crate) mod context_cache;
mod context_transformer;
mod context_version;
pub(crate) mod convert;
mod credential;
pub(crate) mod display;
mod emit;
mod error;
mod event_forwarder;
mod fallback;
mod fn_tool;
mod handle;
#[cfg(feature = "hot-reload")]
pub mod hot_reload;
mod loop_;
pub(crate) mod message_provider;
mod messaging;
pub(crate) mod metrics;
mod model_catalog;
mod model_presets;
mod noop_tool;
mod orchestrator;
#[cfg(feature = "otel")]
pub mod otel;
#[cfg(feature = "plugins")]
pub(crate) mod plugin;
pub(crate) mod policy;
mod registry;
mod retry;
mod schema;
mod state;
pub(crate) mod stream;
mod stream_middleware;
mod sub_agent;
pub(crate) mod tool;
mod tool_execution_policy;
pub(crate) mod tool_filter;
mod tool_middleware;
pub(crate) mod tools;
pub(crate) mod transfer;
pub(crate) mod types;
mod util;

#[cfg(feature = "testkit")]
pub mod testing;

pub mod prelude;

// Re-exports
pub use agent::{
    Agent, AgentOptions, AgentState, DEFAULT_PLAN_MODE_ADDENDUM, FollowUpMode, SteeringMode,
    SubscriptionId, default_convert,
};
pub use agent_id::AgentId;
pub use block_accumulator::{BlockAccumulator, OpenBlock, StreamFinalize, finalize_blocks};
pub use async_context_transformer::{AsyncContextTransformer, AsyncTransformFuture};
pub use checkpoint::{Checkpoint, CheckpointFuture, CheckpointStore, LoopCheckpoint};
pub use config::{
    AgentConfig, ApprovalModeConfig, CacheConfigData, FollowUpModeConfig, RetryConfig,
    SteeringModeConfig, StreamOptionsConfig,
};
pub use context::CompactionReport;
#[allow(deprecated)]
pub use context::{
    DefaultTokenCounter, TokenCounter, estimate_tokens, is_context_overflow, sliding_window,
};
pub use context_cache::{CacheConfig, CacheHint, CacheState};
pub use context_transformer::{ContextTransformer, SlidingWindowTransformer};
pub use context_version::{
    ContextSummarizer, ContextVersion, ContextVersionMeta, ContextVersionStore,
    InMemoryVersionStore, VersioningTransformer,
};
pub use convert::{MessageConverter, ToolSchema, convert_messages, extract_tool_schemas};
pub use credential::{
    AuthConfig, AuthScheme, Credential, CredentialError, CredentialFuture, CredentialResolver,
    CredentialStore, CredentialType, ResolvedCredential,
};
pub use emit::Emission;
pub use error::{AgentError, DowncastError};
pub use event_forwarder::EventForwarderFn;
pub use fallback::ModelFallback;
pub use fn_tool::FnTool;
pub use handle::{AgentHandle, AgentStatus};
#[cfg(feature = "hot-reload")]
pub use hot_reload::{ScriptTool, ToolWatcher};
pub use loop_::{AgentEvent, AgentLoopConfig, TurnEndReason, agent_loop, agent_loop_continue};
pub use message_provider::{
    ChannelMessageProvider, ComposedMessageProvider, MessageProvider, MessageSender, from_fns,
    message_channel,
};
pub use messaging::{AgentMailbox, send_to};
pub use metrics::{MetricsCollector, MetricsFuture, ToolExecMetrics, TurnMetrics};
pub use model_catalog::{
    ApiVersion, AuthMode, CatalogPreset, ModelCatalog, PresetCapability, PresetCatalog,
    PresetStatus, ProviderCatalog, ProviderKind, calculate_cost, model_catalog,
};
pub use model_presets::{ModelConnection, ModelConnections, ModelConnectionsBuilder};
pub use noop_tool::NoopTool;
pub use orchestrator::{
    AgentOrchestrator, AgentRequest, DefaultSupervisor, OrchestratedHandle, SupervisorAction,
    SupervisorPolicy,
};
#[cfg(feature = "otel")]
pub use otel::{OtelInitConfig, init_otel_layer};
pub use registry::{AgentRef, AgentRegistry};
pub use retry::{DefaultRetryStrategy, RetryStrategy};
pub use schema::schema_for;
pub use schemars::JsonSchema;
pub use state::{SessionState, StateDelta};
pub use stream::{
    AssistantMessageDelta, AssistantMessageEvent, CacheStrategy, OnRawPayload, StreamErrorKind,
    StreamFn, StreamOptions, StreamTransport, accumulate_message,
};
pub use stream_middleware::StreamMiddleware;
pub use sub_agent::SubAgent;
pub use tool::{
    AgentTool, AgentToolResult, ApprovalMode, IntoTool, ToolApproval, ToolApprovalRequest,
    ToolFuture, ToolMetadata, ToolParameters, redact_sensitive_values, selective_approve,
    unknown_tool_result, validate_schema, validate_tool_arguments, validation_error_result,
};
pub use tool_execution_policy::{
    PriorityFn, ToolCallSummary, ToolExecutionPolicy, ToolExecutionStrategy,
    ToolExecutionStrategyFuture,
};
pub use tool_filter::{ToolFilter, ToolPattern};
pub use tool_middleware::ToolMiddleware;
#[cfg(feature = "builtin-tools")]
pub use tools::{BashTool, ReadFileTool, WriteFileTool, builtin_tools};
#[cfg(feature = "artifact-tools")]
pub use tools::{ListArtifactsTool, LoadArtifactTool, SaveArtifactTool, artifact_tools};
pub use types::{
    AgentContext, AgentMessage, AgentResult, AssistantMessage, ContentBlock, Cost, CustomMessage,
    CustomMessageRegistry, ImageSource, LlmMessage, MessageSlot, ModelCapabilities, ModelSpec,
    SerializedCustomMessage, SerializedMessages, StopReason, ThinkingBudgets, ThinkingLevel,
    ToolResultMessage, TurnSnapshot, Usage, UserMessage, clone_messages_for_send,
    deserialize_custom_message, restore_messages, restore_single_custom, serialize_custom_message,
    serialize_messages,
};
pub use util::now_timestamp;

#[cfg(feature = "artifact-store")]
pub use artifact::{
    ArtifactByteStream, ArtifactData, ArtifactError, ArtifactMeta, ArtifactStore, ArtifactVersion,
    StreamingArtifactStore, validate_artifact_name,
};
pub use display::{CoreDisplayMessage, DisplayRole, IntoDisplayMessages};
#[cfg(feature = "plugins")]
pub use plugin::{NamespacedTool, Plugin, PluginRegistry};
pub use policy::{
    PolicyContext, PolicyVerdict, PostLoopPolicy, PostTurnPolicy, PreDispatchPolicy,
    PreDispatchVerdict, PreTurnPolicy, ToolDispatchContext, TurnPolicyContext, run_policies,
    run_post_loop_policies, run_post_turn_policies, run_pre_dispatch_policies,
};
#[cfg(feature = "transfer")]
pub use transfer::{TransferChain, TransferError, TransferSignal, TransferToAgentTool};
