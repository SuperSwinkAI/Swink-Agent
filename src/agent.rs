//! Stateful public API wrapper over the agent loop.
//!
//! The [`Agent`] struct owns conversation state, configuration, and queue
//! management. It provides three invocation modes (`prompt_stream`,
//! `prompt_async`, `prompt_sync`), structured output extraction, steering and
//! follow-up queues, and an observer/subscriber pattern for event dispatch.
//!
//! Configuration is split into [`AgentOptions`] (defined in [`crate::agent_options`])
//! and subscription management is in [`crate::agent_subscriptions`].

#[path = "agent/checkpointing.rs"]
mod checkpointing;
#[path = "agent/control.rs"]
mod control;
#[path = "agent/events.rs"]
mod events;
#[path = "agent/invoke.rs"]
mod invoke;
#[path = "agent/mutation.rs"]
mod mutation;
#[path = "agent/queueing.rs"]
mod queueing;
#[path = "agent/state_updates.rs"]
mod state_updates;
#[path = "agent/structured_output.rs"]
mod structured_output;

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::agent_id::AgentId;
use crate::agent_options::{
    ApproveToolArc, AsyncTransformContextArc, CheckpointStoreArc, ConvertToLlmFn, GetApiKeyArc,
    TransformContextArc,
};
use crate::agent_subscriptions::ListenerRegistry;
use crate::error::AgentError;
use crate::message_provider::MessageProvider;
use crate::retry::RetryStrategy;
use crate::stream::{StreamFn, StreamOptions};
use crate::tool::{AgentTool, ApprovalMode};
use crate::types::{AgentMessage, LlmMessage, ModelSpec};

// Re-export so `lib.rs` can still do `pub use agent::{AgentOptions, SubscriptionId, ...}`.
pub use crate::agent_options::{AgentOptions, DEFAULT_PLAN_MODE_ADDENDUM};
pub use crate::agent_subscriptions::SubscriptionId;

// ─── Enums / modes ───────────────────────────────────────────────────────────

/// Controls how steering messages are drained from the queue.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SteeringMode {
    /// Drain all pending steering messages at once.
    All,
    /// Drain one steering message per poll.
    #[default]
    OneAtATime,
}

/// Controls how follow-up messages are drained from the queue.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FollowUpMode {
    /// Drain all pending follow-up messages at once.
    All,
    /// Drain one follow-up message per poll.
    #[default]
    OneAtATime,
}

// ─── AgentState ──────────────────────────────────────────────────────────────

/// Observable state of the agent.
pub struct AgentState {
    /// The system prompt sent to the LLM.
    pub system_prompt: String,
    /// The model specification.
    pub model: ModelSpec,
    /// Available tools.
    pub tools: Vec<Arc<dyn AgentTool>>,
    /// Full conversation history.
    pub messages: Vec<AgentMessage>,
    /// Whether the agent loop is currently executing.
    pub is_running: bool,
    /// The message currently being streamed (if any).
    pub stream_message: Option<AgentMessage>,
    /// Tool call IDs that are currently executing.
    pub pending_tool_calls: HashSet<String>,
    /// Last error from a run, if any.
    pub error: Option<String>,
    /// Available model specifications for model cycling.
    pub available_models: Vec<ModelSpec>,
}

impl std::fmt::Debug for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentState")
            .field("system_prompt", &self.system_prompt)
            .field("model", &self.model)
            .field("tools", &format_args!("[{} tool(s)]", self.tools.len()))
            .field("messages", &self.messages)
            .field("is_running", &self.is_running)
            .field("stream_message", &self.stream_message)
            .field("pending_tool_calls", &self.pending_tool_calls)
            .field("error", &self.error)
            .field("available_models", &self.available_models)
            .finish()
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Default message converter: pass LLM messages through, drop custom messages.
///
/// This is the standard converter for most use cases. Custom messages are
/// filtered out since they are not meant to be sent to the LLM provider.
pub fn default_convert(msg: &AgentMessage) -> Option<LlmMessage> {
    match msg {
        AgentMessage::Llm(llm) => Some(llm.clone()),
        AgentMessage::Custom(_) => None,
    }
}

type ModelStreamRegistry = Vec<(ModelSpec, Arc<dyn StreamFn>)>;

fn available_models_and_stream_fns(
    options: &AgentOptions,
) -> (Vec<ModelSpec>, ModelStreamRegistry) {
    let primary_model = options.model.clone();
    let primary_stream_fn = Arc::clone(&options.stream_fn);
    let mut available_models = vec![options.model.clone()];
    available_models.extend(
        options
            .available_models
            .iter()
            .map(|(model, _): &(ModelSpec, _)| model.clone()),
    );
    let mut model_stream_fns = vec![(primary_model, primary_stream_fn)];
    model_stream_fns.extend(
        options
            .available_models
            .iter()
            .map(|(model, stream_fn): &(ModelSpec, _)| (model.clone(), Arc::clone(stream_fn))),
    );

    (available_models, model_stream_fns)
}

#[cfg(feature = "plugins")]
fn dispatch_plugin_on_init(agent: &Agent) {
    // Dispatch on_init to each plugin in priority order (already sorted).
    // Panics are caught and logged — the plugin's other contributions
    // (policies, tools, event observers) remain active.
    for plugin in &agent.plugins {
        let name = plugin.name().to_owned();
        let plugin_ref = Arc::clone(plugin);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            plugin_ref.on_init(agent);
        }));
        if let Err(cause) = result {
            let msg = cause
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| cause.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_owned());
            tracing::warn!(plugin = %name, error = %msg, "plugin on_init panicked");
        }
    }
}

#[cfg(not(feature = "plugins"))]
const fn dispatch_plugin_on_init(_agent: &Agent) {}
// ─── Agent ───────────────────────────────────────────────────────────────────

/// Stateful wrapper over the agent loop.
///
/// Owns conversation history, configuration, steering/follow-up queues, and
/// subscriber callbacks. Provides prompt, continue, and structured output
/// invocation modes.
pub struct Agent {
    // ── Identity ──
    id: AgentId,

    // ── Public-facing state ──
    state: AgentState,

    // ── Private fields ──
    steering_queue: Arc<Mutex<VecDeque<AgentMessage>>>,
    follow_up_queue: Arc<Mutex<VecDeque<AgentMessage>>>,
    listeners: ListenerRegistry,
    abort_controller: Option<CancellationToken>,
    steering_mode: SteeringMode,
    follow_up_mode: FollowUpMode,
    stream_fn: Arc<dyn StreamFn>,
    convert_to_llm: ConvertToLlmFn,
    transform_context: Option<TransformContextArc>,
    get_api_key: Option<GetApiKeyArc>,
    retry_strategy: Arc<dyn RetryStrategy>,
    stream_options: StreamOptions,
    structured_output_max_retries: usize,
    idle_notify: Arc<Notify>,
    in_flight_llm_messages: Option<Vec<AgentMessage>>,
    in_flight_messages: Option<Vec<AgentMessage>>,
    approve_tool: Option<ApproveToolArc>,
    approval_mode: ApprovalMode,
    pre_turn_policies: Vec<Arc<dyn crate::policy::PreTurnPolicy>>,
    pre_dispatch_policies: Vec<Arc<dyn crate::policy::PreDispatchPolicy>>,
    post_turn_policies: Vec<Arc<dyn crate::policy::PostTurnPolicy>>,
    post_loop_policies: Vec<Arc<dyn crate::policy::PostLoopPolicy>>,
    /// Extra `model/stream_fn` pairs for model cycling.
    model_stream_fns: Vec<(ModelSpec, Arc<dyn StreamFn>)>,
    /// Event forwarders that receive cloned events after listener dispatch.
    event_forwarders: Vec<crate::event_forwarder::EventForwarderFn>,
    /// Optional async context transformer.
    async_transform_context: Option<AsyncTransformContextArc>,
    /// Optional checkpoint store.
    checkpoint_store: Option<CheckpointStoreArc>,
    /// Optional registry for decoding persisted custom messages on restore.
    pub(crate) custom_message_registry: Option<Arc<crate::types::CustomMessageRegistry>>,
    /// Optional metrics collector.
    metrics_collector: Option<Arc<dyn crate::metrics::MetricsCollector>>,
    /// Optional model fallback chain.
    fallback: Option<crate::fallback::ModelFallback>,
    /// Optional external message provider.
    external_message_provider: Option<Arc<dyn MessageProvider>>,
    /// Tool execution policy.
    tool_execution_policy: crate::tool_execution_policy::ToolExecutionPolicy,
    /// Optional plan mode addendum (falls back to `DEFAULT_PLAN_MODE_ADDENDUM`).
    plan_mode_addendum: Option<String>,
    /// Session key-value state store shared with tools and policies.
    session_state: Arc<std::sync::RwLock<crate::SessionState>>,
    /// Optional credential resolver for tool authentication.
    credential_resolver: Option<Arc<dyn crate::credential::CredentialResolver>>,
    /// Optional context caching configuration.
    cache_config: Option<crate::context_cache::CacheConfig>,
    /// Optional dynamic system prompt.
    dynamic_system_prompt: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    /// Shared flag: true while a spawned loop task is active. Set to false by
    /// the `LoopGuardStream` wrapper on drop or by `collect_stream`/`AgentEnd`.
    /// Used by `check_not_running` and `wait_for_idle` as the ground truth for
    /// whether a loop is still in progress (instead of `state.is_running` which
    /// may lag on the stream-drop path).
    loop_active: Arc<AtomicBool>,
    /// Monotonically increasing counter bumped on each `start_loop`. Prevents a
    /// stale `LoopGuardStream` from clearing `loop_active` for a newer run.
    loop_generation: Arc<AtomicU64>,
    /// Registered plugins retained for runtime introspection (priority-sorted).
    #[cfg(feature = "plugins")]
    plugins: Vec<Arc<dyn crate::plugin::Plugin>>,
    /// Optional agent name for transfer chain safety enforcement.
    #[allow(clippy::struct_field_names)]
    agent_name: Option<String>,
    /// Optional transfer chain state carried from a previous handoff.
    transfer_chain: Option<crate::transfer::TransferChain>,
}

impl Agent {
    /// Create a new agent from the given options.
    #[must_use]
    pub fn new(options: AgentOptions) -> Self {
        // Merge plugin contributions (policies, tools, event observers) into options.
        #[cfg(feature = "plugins")]
        let options = merge_plugin_contributions(options);

        // Compute the effective system prompt before partial moves.
        let effective_prompt = options.effective_system_prompt().to_owned();
        let (available_models, model_stream_fns) = available_models_and_stream_fns(&options);

        // If a custom token counter is provided and no custom transform_context
        // was set, rebuild the default SlidingWindowTransformer with the counter.
        let transform_context = match (options.token_counter, options.transform_context) {
            (Some(counter), None) => Some(Arc::new(
                crate::context_transformer::SlidingWindowTransformer::new(100_000, 50_000, 2)
                    .with_token_counter(counter),
            ) as TransformContextArc),
            (_, tc) => tc,
        };

        let agent = Self {
            id: AgentId::next(),
            state: AgentState {
                system_prompt: effective_prompt,
                model: options.model,
                tools: options.tools,
                messages: Vec::new(),
                is_running: false,
                stream_message: None,
                pending_tool_calls: HashSet::new(),
                error: None,
                available_models,
            },
            steering_queue: Arc::new(Mutex::new(VecDeque::new())),
            follow_up_queue: Arc::new(Mutex::new(VecDeque::new())),
            listeners: ListenerRegistry::new(),
            abort_controller: None,
            steering_mode: options.steering_mode,
            follow_up_mode: options.follow_up_mode,
            stream_fn: options.stream_fn,
            convert_to_llm: options.convert_to_llm,
            transform_context,
            get_api_key: options.get_api_key,
            retry_strategy: Arc::from(options.retry_strategy),
            stream_options: options.stream_options,
            structured_output_max_retries: options.structured_output_max_retries,
            idle_notify: Arc::new(Notify::new()),
            in_flight_llm_messages: None,
            in_flight_messages: None,
            approve_tool: options.approve_tool,
            approval_mode: options.approval_mode,
            pre_turn_policies: options.pre_turn_policies,
            pre_dispatch_policies: options.pre_dispatch_policies,
            post_turn_policies: options.post_turn_policies,
            post_loop_policies: options.post_loop_policies,
            model_stream_fns,
            event_forwarders: options.event_forwarders,
            async_transform_context: options.async_transform_context,
            checkpoint_store: options.checkpoint_store,
            custom_message_registry: options.custom_message_registry,
            metrics_collector: options.metrics_collector,
            fallback: options.fallback,
            external_message_provider: options.external_message_provider,
            tool_execution_policy: options.tool_execution_policy,
            plan_mode_addendum: options.plan_mode_addendum,
            session_state: Arc::new(std::sync::RwLock::new(
                options.session_state.unwrap_or_default(),
            )),
            credential_resolver: options.credential_resolver,
            cache_config: options.cache_config,
            dynamic_system_prompt: options.dynamic_system_prompt.map(Arc::from),
            loop_active: Arc::new(AtomicBool::new(false)),
            loop_generation: Arc::new(AtomicU64::new(0)),
            #[cfg(feature = "plugins")]
            plugins: options.plugins,
            agent_name: options.agent_name,
            transfer_chain: options.transfer_chain,
        };

        dispatch_plugin_on_init(&agent);

        agent
    }

    /// Returns this agent's unique identifier.
    #[must_use]
    pub const fn id(&self) -> AgentId {
        self.id
    }

    /// Access the current agent state.
    ///
    /// Note: [`AgentState::is_running`] may lag behind the true loop lifecycle
    /// after a stream is dropped. Use [`Agent::is_running`] for an accurate
    /// real-time check.
    #[must_use]
    pub const fn state(&self) -> &AgentState {
        &self.state
    }

    /// Returns whether a loop is currently active.
    ///
    /// This reads an atomic flag that is cleared immediately when the event
    /// stream is dropped or drained to `AgentEnd`, making it accurate even in
    /// the window between dropping a stream and the next `&mut self` call.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.loop_active.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Access the session key-value state (thread-safe, shared reference).
    #[must_use]
    pub const fn session_state(&self) -> &Arc<std::sync::RwLock<crate::SessionState>> {
        &self.session_state
    }

    /// Access the custom message registry, if one was configured.
    ///
    /// Useful for passing to [`SessionStore::load`](swink_agent_memory) so that
    /// persisted custom messages are deserialized instead of silently dropped.
    #[must_use]
    pub fn custom_message_registry(&self) -> Option<&crate::types::CustomMessageRegistry> {
        self.custom_message_registry.as_deref()
    }

    /// Returns all registered plugins sorted by priority (highest first).
    #[cfg(feature = "plugins")]
    #[must_use]
    pub fn plugins(&self) -> &[Arc<dyn crate::plugin::Plugin>] {
        &self.plugins
    }

    /// Look up a registered plugin by name.
    #[cfg(feature = "plugins")]
    #[must_use]
    pub fn plugin(&self, name: &str) -> Option<&Arc<dyn crate::plugin::Plugin>> {
        self.plugins.iter().find(|p| p.name() == name)
    }
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("state", &self.state)
            .field("steering_mode", &self.steering_mode)
            .field("follow_up_mode", &self.follow_up_mode)
            .field(
                "listeners",
                &format_args!("{} listener(s)", self.listeners.len()),
            )
            .field("is_abort_active", &self.abort_controller.is_some())
            .finish_non_exhaustive()
    }
}

// ─── Plugin merge ───────────────────────────────────────────────────────────

/// Sort plugins by priority and merge their policies, tools, and event
/// observers into the `AgentOptions`. Plugin policies are prepended before
/// directly-registered policies; plugin tools are appended after direct tools
/// (namespaced with the plugin name); plugin event forwarders are prepended.
#[cfg(feature = "plugins")]
fn merge_plugin_contributions(mut options: AgentOptions) -> AgentOptions {
    // Sort plugins by priority descending (stable sort preserves insertion order for ties).
    options
        .plugins
        .sort_by_key(|p| std::cmp::Reverse(p.priority()));

    let mut plugin_pre_turn: Vec<Arc<dyn crate::policy::PreTurnPolicy>> = Vec::new();
    let mut plugin_pre_dispatch: Vec<Arc<dyn crate::policy::PreDispatchPolicy>> = Vec::new();
    let mut plugin_post_turn: Vec<Arc<dyn crate::policy::PostTurnPolicy>> = Vec::new();
    let mut plugin_post_loop: Vec<Arc<dyn crate::policy::PostLoopPolicy>> = Vec::new();
    let mut plugin_tools: Vec<Arc<dyn AgentTool>> = Vec::new();
    let mut plugin_forwarders: Vec<crate::event_forwarder::EventForwarderFn> = Vec::new();

    for plugin in &options.plugins {
        plugin_pre_turn.extend(plugin.pre_turn_policies());
        plugin_pre_dispatch.extend(plugin.pre_dispatch_policies());
        plugin_post_turn.extend(plugin.post_turn_policies());
        plugin_post_loop.extend(plugin.post_loop_policies());

        // Wrap plugin tools in NamespacedTool.
        let plugin_name = plugin.name().to_owned();
        for tool in plugin.tools() {
            plugin_tools.push(Arc::new(crate::plugin::NamespacedTool::new(
                &plugin_name,
                tool,
            )));
        }

        // Wrap plugin's on_event as EventForwarderFn.
        let plugin_ref = Arc::clone(plugin);
        plugin_forwarders.push(Arc::new(move |event: crate::loop_::AgentEvent| {
            plugin_ref.on_event(&event);
        }));
    }

    // Prepend plugin policies before direct policies.
    plugin_pre_turn.append(&mut options.pre_turn_policies);
    options.pre_turn_policies = plugin_pre_turn;

    plugin_pre_dispatch.append(&mut options.pre_dispatch_policies);
    options.pre_dispatch_policies = plugin_pre_dispatch;

    plugin_post_turn.append(&mut options.post_turn_policies);
    options.post_turn_policies = plugin_post_turn;

    plugin_post_loop.append(&mut options.post_loop_policies);
    options.post_loop_policies = plugin_post_loop;

    // Append namespaced plugin tools after direct tools.
    options.tools.extend(plugin_tools);

    // Prepend plugin event forwarders before direct forwarders.
    plugin_forwarders.append(&mut options.event_forwarders);
    options.event_forwarders = plugin_forwarders;

    options
}

// ─── SharedRetryStrategy ─────────────────────────────────────────────────────

/// Wrapper that delegates to an `Arc<dyn RetryStrategy>`, allowing
/// the agent to share its retry strategy with each loop config.
struct SharedRetryStrategy(Arc<dyn RetryStrategy>);

impl RetryStrategy for SharedRetryStrategy {
    fn should_retry(&self, error: &AgentError, attempt: u32) -> bool {
        self.0.should_retry(error, attempt)
    }

    fn delay(&self, attempt: u32) -> std::time::Duration {
        self.0.delay(attempt)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
