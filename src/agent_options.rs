//! Configuration options for constructing an [`Agent`](crate::Agent).
//!
//! [`AgentOptions`] is the single entry point for wiring up an agent: it
//! collects the model spec, stream function, tools, hooks, and policies that
//! the agent loop needs.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::async_context_transformer::AsyncContextTransformer;
use crate::checkpoint::CheckpointStore;
use crate::loop_::AgentEvent;
use crate::message_provider::MessageProvider;
use crate::retry::{DefaultRetryStrategy, RetryStrategy};
use crate::stream::{StreamFn, StreamOptions};
use crate::tool::{AgentTool, ApprovalMode, ToolApproval, ToolApprovalRequest};
use crate::types::{AgentMessage, LlmMessage, ModelSpec};

// ─── Type aliases (module-local) ─────────────────────────────────────────────

pub(crate) type ConvertToLlmFn = Arc<dyn Fn(&AgentMessage) -> Option<LlmMessage> + Send + Sync>;
pub(crate) type TransformContextArc = Arc<dyn crate::context_transformer::ContextTransformer>;
pub(crate) type AsyncTransformContextArc = Arc<dyn AsyncContextTransformer>;
pub(crate) type CheckpointStoreArc = Arc<dyn CheckpointStore>;
pub(crate) type GetApiKeyFn =
    Arc<dyn Fn(&str) -> Pin<Box<dyn Future<Output = Option<String>> + Send>> + Send + Sync>;
pub(crate) type ApproveToolArc = Arc<crate::loop_::ApproveToolFn>;

// ─── Plan mode addendum ───────────────────────────────────────────────────────

/// Fallback addendum appended in plan mode when no custom addendum is set.
pub const DEFAULT_PLAN_MODE_ADDENDUM: &str = "\n\nYou are in planning mode. Analyze the request and produce a step-by-step plan. Do not make any modifications or execute any write operations.";

// ─── AgentOptions ─────────────────────────────────────────────────────────────

/// Configuration options for constructing an [`Agent`](crate::Agent).
pub struct AgentOptions {
    /// System prompt.
    pub system_prompt: String,
    /// Model specification.
    pub model: ModelSpec,
    /// Available tools.
    pub tools: Vec<Arc<dyn AgentTool>>,
    /// The streaming function implementation.
    pub stream_fn: Arc<dyn StreamFn>,
    /// Converts agent messages to LLM messages (filter custom messages).
    pub convert_to_llm: ConvertToLlmFn,
    /// Optional context transformer.
    pub transform_context: Option<TransformContextArc>,
    /// Optional async API key resolver.
    pub get_api_key: Option<GetApiKeyFn>,
    /// Retry strategy for transient failures.
    pub retry_strategy: Box<dyn RetryStrategy>,
    /// Per-call stream options.
    pub stream_options: StreamOptions,
    /// Steering queue drain mode.
    pub steering_mode: crate::agent::SteeringMode,
    /// Follow-up queue drain mode.
    pub follow_up_mode: crate::agent::FollowUpMode,
    /// Max retries for structured output validation.
    pub structured_output_max_retries: usize,
    /// Optional async callback for approving/rejecting tool calls before execution.
    pub approve_tool: Option<ApproveToolArc>,
    /// Controls whether the approval gate is active. Defaults to `Enabled`.
    pub approval_mode: ApprovalMode,
    /// Additional model specs for model cycling (each with its own stream function).
    pub available_models: Vec<(ModelSpec, Arc<dyn StreamFn>)>,
    /// Pre-turn policies evaluated before each LLM call.
    pub pre_turn_policies: Vec<Arc<dyn crate::policy::PreTurnPolicy>>,
    /// Pre-dispatch policies evaluated per tool call, before approval.
    pub pre_dispatch_policies: Vec<Arc<dyn crate::policy::PreDispatchPolicy>>,
    /// Post-turn policies evaluated after each completed turn.
    pub post_turn_policies: Vec<Arc<dyn crate::policy::PostTurnPolicy>>,
    /// Post-loop policies evaluated after the inner loop exits.
    pub post_loop_policies: Vec<Arc<dyn crate::policy::PostLoopPolicy>>,
    /// Event forwarders that receive all dispatched events.
    pub event_forwarders: Vec<crate::event_forwarder::EventForwarderFn>,
    /// Optional async context transformer (runs before the sync transformer).
    pub async_transform_context: Option<AsyncTransformContextArc>,
    /// Optional checkpoint store for persisting agent state.
    pub checkpoint_store: Option<CheckpointStoreArc>,
    /// Optional metrics collector for per-turn observability.
    pub metrics_collector: Option<Arc<dyn crate::metrics::MetricsCollector>>,
    /// Optional custom token counter for context compaction.
    ///
    /// When set, the default [`SlidingWindowTransformer`](crate::SlidingWindowTransformer)
    /// uses this counter instead of the `chars / 4` heuristic. Has no effect if a
    /// custom `transform_context` is provided (use
    /// [`SlidingWindowTransformer::with_token_counter`](crate::SlidingWindowTransformer::with_token_counter)
    /// directly in that case).
    pub token_counter: Option<Arc<dyn crate::context::TokenCounter>>,
    /// Optional model fallback chain tried when the primary model exhausts
    /// its retry budget on a retryable error.
    pub fallback: Option<crate::fallback::ModelFallback>,
    /// Optional external message provider composed with the internal queues.
    ///
    /// Set via [`with_message_channel`](Self::with_message_channel) or
    /// [`with_external_message_provider`](Self::with_external_message_provider).
    pub external_message_provider: Option<Arc<dyn MessageProvider>>,
    /// Controls how tool calls within a turn are dispatched.
    ///
    /// Defaults to [`Concurrent`](crate::tool_execution_policy::ToolExecutionPolicy::Concurrent).
    pub tool_execution_policy: crate::tool_execution_policy::ToolExecutionPolicy,
    /// Optional addendum appended to the system prompt when entering plan mode.
    ///
    /// Falls back to [`DEFAULT_PLAN_MODE_ADDENDUM`] when `None`.
    pub plan_mode_addendum: Option<String>,
}

impl AgentOptions {
    /// Create options with required fields and sensible defaults.
    #[must_use]
    pub fn new(
        system_prompt: impl Into<String>,
        model: ModelSpec,
        stream_fn: Arc<dyn StreamFn>,
        convert_to_llm: impl Fn(&AgentMessage) -> Option<LlmMessage> + Send + Sync + 'static,
    ) -> Self {
        Self {
            system_prompt: system_prompt.into(),
            model,
            tools: Vec::new(),
            stream_fn,
            convert_to_llm: Arc::new(convert_to_llm),
            transform_context: Some(Arc::new(
                crate::context_transformer::SlidingWindowTransformer::new(100_000, 50_000, 2),
            )),
            get_api_key: None,
            retry_strategy: Box::new(DefaultRetryStrategy::default()),
            stream_options: StreamOptions::default(),
            steering_mode: crate::agent::SteeringMode::default(),
            follow_up_mode: crate::agent::FollowUpMode::default(),
            structured_output_max_retries: 3,
            approve_tool: None,
            approval_mode: ApprovalMode::default(),
            available_models: Vec::new(),
            pre_turn_policies: Vec::new(),
            pre_dispatch_policies: Vec::new(),
            post_turn_policies: Vec::new(),
            post_loop_policies: Vec::new(),
            event_forwarders: Vec::new(),
            async_transform_context: None,
            checkpoint_store: None,
            metrics_collector: None,
            token_counter: None,
            fallback: None,
            external_message_provider: None,
            tool_execution_policy: crate::tool_execution_policy::ToolExecutionPolicy::default(),
            plan_mode_addendum: None,
        }
    }

    /// Simplified constructor using [`default_convert`](crate::default_convert) and sensible defaults.
    ///
    /// Equivalent to `AgentOptions::new(system_prompt, model, stream_fn, default_convert)`.
    #[must_use]
    pub fn new_simple(
        system_prompt: impl Into<String>,
        model: ModelSpec,
        stream_fn: Arc<dyn StreamFn>,
    ) -> Self {
        Self::new(
            system_prompt,
            model,
            stream_fn,
            crate::agent::default_convert,
        )
    }

    /// Build options directly from a [`ModelConnections`](crate::ModelConnections) bundle.
    ///
    /// This avoids the manual `into_parts()` decomposition. The primary model
    /// and stream function are extracted, and any extra models are set as
    /// available models for cycling.
    #[must_use]
    pub fn from_connections(
        system_prompt: impl Into<String>,
        connections: crate::model_presets::ModelConnections,
    ) -> Self {
        let (model, stream_fn, extra_models) = connections.into_parts();
        Self::new_simple(system_prompt, model, stream_fn).with_available_models(extra_models)
    }

    /// Set the available tools.
    #[must_use]
    pub fn with_tools(mut self, tools: Vec<Arc<dyn AgentTool>>) -> Self {
        self.tools = tools;
        self
    }

    /// Convenience: register all built-in tools (bash, read-file, write-file).
    #[cfg(feature = "builtin-tools")]
    #[must_use]
    pub fn with_default_tools(self) -> Self {
        self.with_tools(crate::tools::builtin_tools())
    }

    /// Set the retry strategy.
    #[must_use]
    pub fn with_retry_strategy(mut self, strategy: Box<dyn RetryStrategy>) -> Self {
        self.retry_strategy = strategy;
        self
    }

    /// Set the stream options.
    #[must_use]
    pub fn with_stream_options(mut self, options: StreamOptions) -> Self {
        self.stream_options = options;
        self
    }

    /// Set the context transformer.
    #[must_use]
    pub fn with_transform_context(
        mut self,
        transformer: impl crate::context_transformer::ContextTransformer + 'static,
    ) -> Self {
        self.transform_context = Some(Arc::new(transformer));
        self
    }

    /// Set the context transform hook using a closure.
    ///
    /// Backward-compatible with the old closure-based API. The closure
    /// receives `(&mut Vec<AgentMessage>, bool)` where the bool is the overflow signal.
    #[must_use]
    pub fn with_transform_context_fn(
        mut self,
        f: impl Fn(&mut Vec<AgentMessage>, bool) + Send + Sync + 'static,
    ) -> Self {
        self.transform_context = Some(Arc::new(f));
        self
    }

    /// Set the API key resolver.
    #[must_use]
    pub fn with_get_api_key(
        mut self,
        f: impl Fn(&str) -> Pin<Box<dyn Future<Output = Option<String>> + Send>> + Send + Sync + 'static,
    ) -> Self {
        self.get_api_key = Some(Arc::new(f));
        self
    }

    /// Set the steering mode.
    #[must_use]
    pub const fn with_steering_mode(mut self, mode: crate::agent::SteeringMode) -> Self {
        self.steering_mode = mode;
        self
    }

    /// Set the follow-up mode.
    #[must_use]
    pub const fn with_follow_up_mode(mut self, mode: crate::agent::FollowUpMode) -> Self {
        self.follow_up_mode = mode;
        self
    }

    /// Set the max retries for structured output.
    #[must_use]
    pub const fn with_structured_output_max_retries(mut self, n: usize) -> Self {
        self.structured_output_max_retries = n;
        self
    }

    /// Set the tool approval callback.
    #[must_use]
    pub fn with_approve_tool(
        mut self,
        f: impl Fn(ToolApprovalRequest) -> Pin<Box<dyn Future<Output = ToolApproval> + Send>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        self.approve_tool = Some(Arc::new(f));
        self
    }

    /// Sets the tool approval callback using an async closure.
    ///
    /// This is a convenience wrapper around [`with_approve_tool`](Self::with_approve_tool)
    /// that avoids the `Pin<Box<dyn Future>>` return type ceremony.
    #[must_use]
    pub fn with_approve_tool_async<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(ToolApprovalRequest) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ToolApproval> + Send + 'static,
    {
        let f = std::sync::Arc::new(f);
        self.approve_tool = Some(std::sync::Arc::new(move |req| {
            let f = std::sync::Arc::clone(&f);
            Box::pin(async move { f(req).await })
        }));
        self
    }

    /// Set the approval mode.
    #[must_use]
    pub const fn with_approval_mode(mut self, mode: ApprovalMode) -> Self {
        self.approval_mode = mode;
        self
    }

    /// Set additional models for model cycling.
    #[must_use]
    pub fn with_available_models(mut self, models: Vec<(ModelSpec, Arc<dyn StreamFn>)>) -> Self {
        self.available_models = models;
        self
    }

    /// Add a pre-turn policy (evaluated before each LLM call).
    #[must_use]
    pub fn with_pre_turn_policy(
        mut self,
        policy: impl crate::policy::PreTurnPolicy + 'static,
    ) -> Self {
        self.pre_turn_policies.push(Arc::new(policy));
        self
    }

    /// Add a pre-dispatch policy (evaluated per tool call, before approval).
    #[must_use]
    pub fn with_pre_dispatch_policy(
        mut self,
        policy: impl crate::policy::PreDispatchPolicy + 'static,
    ) -> Self {
        self.pre_dispatch_policies.push(Arc::new(policy));
        self
    }

    /// Add a post-turn policy (evaluated after each completed turn).
    #[must_use]
    pub fn with_post_turn_policy(
        mut self,
        policy: impl crate::policy::PostTurnPolicy + 'static,
    ) -> Self {
        self.post_turn_policies.push(Arc::new(policy));
        self
    }

    /// Add a post-loop policy (evaluated after the inner loop exits).
    #[must_use]
    pub fn with_post_loop_policy(
        mut self,
        policy: impl crate::policy::PostLoopPolicy + 'static,
    ) -> Self {
        self.post_loop_policies.push(Arc::new(policy));
        self
    }

    /// Add an event forwarder that receives all events dispatched by this agent.
    #[must_use]
    pub fn with_event_forwarder(mut self, f: impl Fn(AgentEvent) + Send + Sync + 'static) -> Self {
        self.event_forwarders.push(Arc::new(f));
        self
    }

    /// Set the async context transformer (runs before the sync transformer).
    #[must_use]
    pub fn with_async_transform_context(
        mut self,
        transformer: impl AsyncContextTransformer + 'static,
    ) -> Self {
        self.async_transform_context = Some(Arc::new(transformer));
        self
    }

    /// Set the checkpoint store for persisting agent state.
    #[must_use]
    pub fn with_checkpoint_store(mut self, store: impl CheckpointStore + 'static) -> Self {
        self.checkpoint_store = Some(Arc::new(store));
        self
    }

    /// Set the metrics collector for per-turn observability.
    #[must_use]
    pub fn with_metrics_collector(
        mut self,
        collector: impl crate::metrics::MetricsCollector + 'static,
    ) -> Self {
        self.metrics_collector = Some(Arc::new(collector));
        self
    }

    /// Set a custom token counter for context compaction.
    ///
    /// Replaces the default `chars / 4` heuristic used by the built-in
    /// [`SlidingWindowTransformer`](crate::SlidingWindowTransformer). Supply a
    /// tiktoken or provider-native tokenizer for accurate budget enforcement.
    #[must_use]
    pub fn with_token_counter(
        mut self,
        counter: impl crate::context::TokenCounter + 'static,
    ) -> Self {
        self.token_counter = Some(Arc::new(counter));
        self
    }

    /// Set the model fallback chain.
    ///
    /// When the primary model exhausts its retry budget on a retryable error,
    /// each fallback model is tried in order (with a fresh retry budget)
    /// before the error is surfaced.
    #[must_use]
    pub fn with_model_fallback(mut self, fallback: crate::fallback::ModelFallback) -> Self {
        self.fallback = Some(fallback);
        self
    }

    /// Attach a push-based message channel and return the sender handle.
    ///
    /// Creates a [`ChannelMessageProvider`](crate::ChannelMessageProvider) that
    /// is composed with the agent's internal steering/follow-up queues. External
    /// code can push messages via the returned [`MessageSender`](crate::MessageSender).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut opts = AgentOptions::new_simple("prompt", model, stream_fn);
    /// let sender = opts.with_message_channel();
    /// // later, from another task:
    /// sender.send(user_msg("follow-up directive"));
    /// ```
    pub fn with_message_channel(&mut self) -> crate::message_provider::MessageSender {
        let (provider, sender) = crate::message_provider::message_channel();
        self.external_message_provider = Some(Arc::new(provider));
        sender
    }

    /// Set an external [`MessageProvider`] to compose with the internal queues.
    ///
    /// For push-based messaging, prefer [`with_message_channel`](Self::with_message_channel).
    #[must_use]
    pub fn with_external_message_provider(
        mut self,
        provider: impl MessageProvider + 'static,
    ) -> Self {
        self.external_message_provider = Some(Arc::new(provider));
        self
    }

    /// Set the tool execution policy.
    ///
    /// Controls whether tool calls are dispatched concurrently (default),
    /// sequentially, by priority, or via a fully custom strategy.
    #[must_use]
    pub fn with_tool_execution_policy(
        mut self,
        policy: crate::tool_execution_policy::ToolExecutionPolicy,
    ) -> Self {
        self.tool_execution_policy = policy;
        self
    }

    /// Override the system prompt addendum appended when entering plan mode.
    ///
    /// When not set, [`DEFAULT_PLAN_MODE_ADDENDUM`] is used.
    #[must_use]
    pub fn with_plan_mode_addendum(mut self, addendum: impl Into<String>) -> Self {
        self.plan_mode_addendum = Some(addendum.into());
        self
    }
}
