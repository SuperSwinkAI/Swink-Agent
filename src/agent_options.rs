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
    /// Optional custom validation hook.
    pub tool_validator: Option<Arc<dyn crate::tool_validator::ToolValidator>>,
    /// Additional model specs for model cycling (each with its own stream function).
    pub available_models: Vec<(ModelSpec, Arc<dyn StreamFn>)>,
    /// Optional loop policy.
    pub loop_policy: Option<Arc<dyn crate::loop_policy::LoopPolicy>>,
    /// Optional pre-execution argument transformer.
    pub tool_call_transformer: Option<Arc<dyn crate::tool_call_transformer::ToolCallTransformer>>,
    /// Optional post-turn lifecycle hook.
    pub post_turn_hook: Option<Arc<dyn crate::post_turn_hook::PostTurnHook>>,
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
    /// Optional pre-call budget guard for mid-turn cost/token gating.
    ///
    /// Checked before each LLM call. Complements [`CostCapPolicy`](crate::CostCapPolicy)
    /// which only checks after turns.
    pub budget_guard: Option<crate::budget_guard::BudgetGuard>,
    /// Controls how tool calls within a turn are dispatched.
    ///
    /// Defaults to [`ToolExecutionPolicy::Concurrent`].
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
            tool_validator: None,
            available_models: Vec::new(),
            loop_policy: None,
            tool_call_transformer: None,
            post_turn_hook: None,
            event_forwarders: Vec::new(),
            async_transform_context: None,
            checkpoint_store: None,
            metrics_collector: None,
            token_counter: None,
            fallback: None,
            external_message_provider: None,
            budget_guard: None,
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

    /// Set the approval mode.
    #[must_use]
    pub const fn with_approval_mode(mut self, mode: ApprovalMode) -> Self {
        self.approval_mode = mode;
        self
    }

    /// Set the custom tool validator.
    #[must_use]
    pub fn with_tool_validator(
        mut self,
        validator: impl crate::tool_validator::ToolValidator + 'static,
    ) -> Self {
        self.tool_validator = Some(Arc::new(validator));
        self
    }

    /// Set additional models for model cycling.
    #[must_use]
    pub fn with_available_models(mut self, models: Vec<(ModelSpec, Arc<dyn StreamFn>)>) -> Self {
        self.available_models = models;
        self
    }

    /// Set the pre-execution tool-call argument transformer.
    #[must_use]
    pub fn with_tool_call_transformer(
        mut self,
        transformer: impl crate::tool_call_transformer::ToolCallTransformer + 'static,
    ) -> Self {
        self.tool_call_transformer = Some(Arc::new(transformer));
        self
    }

    /// Set the loop policy for controlling agent loop continuation.
    #[must_use]
    pub fn with_loop_policy(
        mut self,
        policy: impl crate::loop_policy::LoopPolicy + 'static,
    ) -> Self {
        self.loop_policy = Some(Arc::new(policy));
        self
    }

    /// Set the post-turn lifecycle hook.
    #[must_use]
    pub fn with_post_turn_hook(
        mut self,
        hook: impl crate::post_turn_hook::PostTurnHook + 'static,
    ) -> Self {
        self.post_turn_hook = Some(Arc::new(hook));
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

    /// Set a pre-call budget guard for mid-turn cost/token gating.
    ///
    /// The guard is checked before each LLM call. If accumulated cost or
    /// token usage exceeds the configured limits, the loop stops gracefully.
    /// This complements [`CostCapPolicy`](crate::CostCapPolicy) which only
    /// checks after turns complete.
    #[must_use]
    pub const fn with_budget_guard(mut self, guard: crate::budget_guard::BudgetGuard) -> Self {
        self.budget_guard = Some(guard);
        self
    }

    /// Convenience: set a maximum cost limit (creates a [`BudgetGuard`](crate::BudgetGuard)).
    ///
    /// Equivalent to `with_budget_guard(BudgetGuard::new().with_max_cost(max_cost))`.
    #[must_use]
    pub const fn with_cost_limit(self, max_cost: f64) -> Self {
        self.with_budget_guard(crate::budget_guard::BudgetGuard::new().with_max_cost(max_cost))
    }

    /// Convenience: set a maximum token limit (creates a [`BudgetGuard`](crate::BudgetGuard)).
    ///
    /// Equivalent to `with_budget_guard(BudgetGuard::new().with_max_tokens(max_tokens))`.
    #[must_use]
    pub const fn with_token_limit(self, max_tokens: u64) -> Self {
        self.with_budget_guard(crate::budget_guard::BudgetGuard::new().with_max_tokens(max_tokens))
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
