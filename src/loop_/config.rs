//! Configuration for the agent loop.

use std::sync::Arc;

use crate::agent_options::{ApproveToolFn, GetApiKeyFn};
use crate::async_context_transformer::AsyncContextTransformer;
use crate::fallback::ModelFallback;
use crate::message_provider::MessageProvider;
use crate::retry::RetryStrategy;
use crate::stream::{StreamFn, StreamOptions};
use crate::tool::{AgentTool, ApprovalMode};
use crate::tool_execution_policy::ToolExecutionPolicy;
use crate::types::ModelSpec;

use super::ConvertToLlmFn;

// ─── AgentLoopConfig ─────────────────────────────────────────────────────────

/// Configuration for the agent loop.
///
/// Carries the model spec, stream options, retry strategy, stream function,
/// tools, and all the hooks that the loop calls at various points.
pub struct AgentLoopConfig {
    /// Model specification passed through to `StreamFn`.
    pub model: ModelSpec,

    /// Stream options passed through to `StreamFn`.
    pub stream_options: StreamOptions,

    /// Retry strategy applied to model calls.
    pub retry_strategy: Box<dyn RetryStrategy>,

    /// The pluggable streaming function that calls the LLM provider.
    pub stream_fn: Arc<dyn StreamFn>,

    /// Available tools for the agent to call.
    pub tools: Vec<Arc<dyn AgentTool>>,

    /// Converts an `AgentMessage` to an `LlmMessage` for the provider.
    /// Returns `None` to filter out custom or UI-only messages.
    pub convert_to_llm: Box<ConvertToLlmFn>,

    /// Optional hook called before `convert_to_llm`; used for context pruning,
    /// token budget enforcement, or external context injection.
    /// When the overflow signal is set, the transformer should prune more
    /// aggressively.
    pub transform_context: Option<Arc<dyn crate::context_transformer::ContextTransformer>>,

    /// Optional async callback for dynamic API key resolution.
    pub get_api_key: Option<Box<GetApiKeyFn>>,

    /// Optional provider polled for steering and follow-up messages.
    ///
    /// [`MessageProvider::poll_steering`] is called after each tool execution batch.
    /// [`MessageProvider::poll_follow_up`] is called when the agent would otherwise stop.
    pub message_provider: Option<Arc<dyn MessageProvider>>,

    /// Optional async callback for approving/rejecting tool calls before execution.
    /// When `Some` and `approval_mode` is `Enabled`, each tool call is sent through
    /// this callback before dispatch. Rejected tools return an error result to the LLM.
    pub approve_tool: Option<Box<ApproveToolFn>>,

    /// Controls whether the approval gate is active. Defaults to `Enabled`.
    pub approval_mode: ApprovalMode,

    /// Pre-turn policies evaluated before each LLM call.
    pub pre_turn_policies: Vec<Arc<dyn crate::policy::PreTurnPolicy>>,

    /// Pre-dispatch policies evaluated per tool call, before approval.
    pub pre_dispatch_policies: Vec<Arc<dyn crate::policy::PreDispatchPolicy>>,

    /// Post-turn policies evaluated after each completed turn.
    pub post_turn_policies: Vec<Arc<dyn crate::policy::PostTurnPolicy>>,

    /// Post-loop policies evaluated after the inner loop exits.
    pub post_loop_policies: Vec<Arc<dyn crate::policy::PostLoopPolicy>>,

    /// Optional async context transformer (runs before the sync transformer).
    ///
    /// Enables async operations like fetching summaries or RAG retrieval
    /// before context compaction.
    pub async_transform_context: Option<Arc<dyn AsyncContextTransformer>>,

    /// Optional metrics collector invoked at the end of each turn with
    /// per-turn timing, token usage, and cost data.
    pub metrics_collector: Option<Arc<dyn crate::metrics::MetricsCollector>>,

    /// Optional model fallback chain tried when the primary model exhausts
    /// its retry budget on a retryable error.
    pub fallback: Option<ModelFallback>,

    /// Controls how tool calls within a turn are dispatched.
    ///
    /// Defaults to [`ToolExecutionPolicy::Concurrent`] for backward
    /// compatibility.
    pub tool_execution_policy: ToolExecutionPolicy,

    /// Session key-value state store shared with tools and policies.
    pub session_state: Arc<std::sync::RwLock<crate::SessionState>>,

    /// Optional credential resolver for tool authentication.
    pub credential_resolver: Option<Arc<dyn crate::credential::CredentialResolver>>,

    /// Optional context caching configuration.
    pub cache_config: Option<crate::context_cache::CacheConfig>,

    /// Mutable cache state tracking turns since last write.
    pub cache_state: std::sync::Mutex<crate::context_cache::CacheState>,

    /// Optional dynamic system prompt closure (called fresh each turn).
    ///
    /// Its output is injected as a user-role message after the system prompt
    /// to avoid invalidating provider-side caches.
    pub dynamic_system_prompt: Option<Arc<dyn Fn() -> String + Send + Sync>>,
}

impl std::fmt::Debug for AgentLoopConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentLoopConfig")
            .field("model", &self.model)
            .field("stream_options", &self.stream_options)
            .field("tools", &format_args!("[{} tool(s)]", self.tools.len()))
            .field(
                "pre_turn_policies",
                &format_args!("[{} policy(ies)]", self.pre_turn_policies.len()),
            )
            .field(
                "pre_dispatch_policies",
                &format_args!("[{} policy(ies)]", self.pre_dispatch_policies.len()),
            )
            .field(
                "post_turn_policies",
                &format_args!("[{} policy(ies)]", self.post_turn_policies.len()),
            )
            .field(
                "post_loop_policies",
                &format_args!("[{} policy(ies)]", self.post_loop_policies.len()),
            )
            .field("tool_execution_policy", &self.tool_execution_policy)
            .finish_non_exhaustive()
    }
}
