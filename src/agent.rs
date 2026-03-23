//! Stateful public API wrapper over the agent loop.
//!
//! The [`Agent`] struct owns conversation state, configuration, and queue
//! management. It provides three invocation modes (`prompt_stream`,
//! `prompt_async`, `prompt_sync`), structured output extraction, steering and
//! follow-up queues, and an observer/subscriber pattern for event dispatch.
//!
//! Configuration is split into [`AgentOptions`] (defined in [`crate::agent_options`])
//! and subscription management is in [`crate::agent_subscriptions`].

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use futures::{Stream, StreamExt};
use serde_json::Value;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::agent_options::{
    ApproveToolArc, AsyncTransformContextArc, CheckpointStoreArc, ConvertToLlmFn, GetApiKeyFn,
    TransformContextArc,
};
use crate::agent_subscriptions::ListenerRegistry;
use crate::checkpoint::{Checkpoint, CheckpointStore};
use crate::error::AgentError;
use crate::loop_::ApproveToolFn;
use crate::loop_::{AgentEvent, AgentLoopConfig, agent_loop, agent_loop_continue};
use crate::message_provider::MessageProvider;
use crate::registry::AgentId;
use crate::retry::RetryStrategy;
use crate::stream::{StreamFn, StreamOptions};
use crate::tool::{AgentTool, ApprovalMode};
use crate::types::{
    AgentMessage, AgentResult, ContentBlock, Cost, LlmMessage, ModelSpec, StopReason,
    ThinkingLevel, Usage,
};
use crate::util::now_timestamp;

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
    steering_queue: Arc<Mutex<Vec<AgentMessage>>>,
    follow_up_queue: Arc<Mutex<Vec<AgentMessage>>>,
    listeners: ListenerRegistry,
    abort_controller: Option<CancellationToken>,
    steering_mode: SteeringMode,
    follow_up_mode: FollowUpMode,
    stream_fn: Arc<dyn StreamFn>,
    convert_to_llm: ConvertToLlmFn,
    transform_context: Option<TransformContextArc>,
    get_api_key: Option<GetApiKeyFn>,
    retry_strategy: Arc<dyn RetryStrategy>,
    stream_options: StreamOptions,
    structured_output_max_retries: usize,
    idle_notify: Arc<Notify>,
    in_flight_llm_messages: Option<Vec<AgentMessage>>,
    approve_tool: Option<ApproveToolArc>,
    approval_mode: ApprovalMode,
    tool_validator: Option<Arc<dyn crate::tool_validator::ToolValidator>>,
    loop_policy: Option<Arc<dyn crate::loop_policy::LoopPolicy>>,
    tool_call_transformer: Option<Arc<dyn crate::tool_call_transformer::ToolCallTransformer>>,
    post_turn_hook: Option<Arc<dyn crate::post_turn_hook::PostTurnHook>>,
    /// Extra `model/stream_fn` pairs for model cycling.
    model_stream_fns: Vec<(ModelSpec, Arc<dyn StreamFn>)>,
    /// Event forwarders that receive cloned events after listener dispatch.
    event_forwarders: Vec<crate::event_forwarder::EventForwarderFn>,
    /// Optional async context transformer.
    async_transform_context: Option<AsyncTransformContextArc>,
    /// Optional checkpoint store.
    checkpoint_store: Option<CheckpointStoreArc>,
    /// Optional metrics collector.
    metrics_collector: Option<Arc<dyn crate::metrics::MetricsCollector>>,
    /// Optional model fallback chain.
    fallback: Option<crate::fallback::ModelFallback>,
    /// Optional external message provider.
    external_message_provider: Option<Arc<dyn MessageProvider>>,
    /// Optional budget guard.
    budget_guard: Option<crate::budget_guard::BudgetGuard>,
    /// Tool execution policy.
    tool_execution_policy: crate::tool_execution_policy::ToolExecutionPolicy,
    /// Optional plan mode addendum (falls back to `DEFAULT_PLAN_MODE_ADDENDUM`).
    plan_mode_addendum: Option<String>,
}

impl Agent {
    /// Create a new agent from the given options.
    #[must_use]
    pub fn new(options: AgentOptions) -> Self {
        let primary_model = options.model.clone();
        let primary_stream_fn = Arc::clone(&options.stream_fn);
        let mut available_models = vec![options.model.clone()];
        available_models.extend(
            options
                .available_models
                .iter()
                .map(|(m, _): &(ModelSpec, _)| m.clone()),
        );
        let mut model_stream_fns = vec![(primary_model, primary_stream_fn)];
        model_stream_fns.extend(
            options
                .available_models
                .iter()
                .map(|(model, stream_fn): &(ModelSpec, _)| (model.clone(), Arc::clone(stream_fn))),
        );

        // If a custom token counter is provided and no custom transform_context
        // was set, rebuild the default SlidingWindowTransformer with the counter.
        let transform_context = match (options.token_counter, options.transform_context) {
            (Some(counter), None) => Some(Arc::new(
                crate::context_transformer::SlidingWindowTransformer::new(100_000, 50_000, 2)
                    .with_token_counter(counter),
            ) as TransformContextArc),
            (_, tc) => tc,
        };

        Self {
            id: AgentId::next(),
            state: AgentState {
                system_prompt: options.system_prompt,
                model: options.model,
                tools: options.tools,
                messages: Vec::new(),
                is_running: false,
                stream_message: None,
                pending_tool_calls: HashSet::new(),
                error: None,
                available_models,
            },
            steering_queue: Arc::new(Mutex::new(Vec::new())),
            follow_up_queue: Arc::new(Mutex::new(Vec::new())),
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
            approve_tool: options.approve_tool,
            approval_mode: options.approval_mode,
            tool_validator: options.tool_validator,
            loop_policy: options.loop_policy,
            tool_call_transformer: options.tool_call_transformer,
            post_turn_hook: options.post_turn_hook,
            model_stream_fns,
            event_forwarders: options.event_forwarders,
            async_transform_context: options.async_transform_context,
            checkpoint_store: options.checkpoint_store,
            metrics_collector: options.metrics_collector,
            fallback: options.fallback,
            external_message_provider: options.external_message_provider,
            budget_guard: options.budget_guard,
            tool_execution_policy: options.tool_execution_policy,
            plan_mode_addendum: options.plan_mode_addendum,
        }
    }

    /// Returns this agent's unique identifier.
    #[must_use]
    pub const fn id(&self) -> AgentId {
        self.id
    }

    /// Access the current agent state.
    #[must_use]
    pub const fn state(&self) -> &AgentState {
        &self.state
    }

    // ── State Mutation ───────────────────────────────────────────────────

    /// Set the system prompt.
    pub fn set_system_prompt(&mut self, prompt: impl Into<String>) {
        self.state.system_prompt = prompt.into();
    }

    /// Set the model specification, swapping the stream function if a matching
    /// model was registered via [`with_available_models`](AgentOptions::with_available_models).
    pub fn set_model(&mut self, model: ModelSpec) {
        if let Some((_, sfn)) = self.model_stream_fns.iter().find(|(m, _)| *m == model) {
            self.stream_fn = Arc::clone(sfn);
        }
        self.state.model = model;
    }

    /// Set the thinking level on the current model.
    pub const fn set_thinking_level(&mut self, level: ThinkingLevel) {
        self.state.model.thinking_level = level;
    }

    /// Replace the tool set.
    pub fn set_tools(&mut self, tools: Vec<Arc<dyn AgentTool>>) {
        self.state.tools = tools;
    }

    /// Add a tool, replacing any existing tool with the same name.
    pub fn add_tool(&mut self, tool: Arc<dyn AgentTool>) {
        let name = tool.name();
        self.state.tools.retain(|t| t.name() != name);
        self.state.tools.push(tool);
    }

    /// Remove a tool by name. Returns `true` if a tool was found and removed.
    pub fn remove_tool(&mut self, name: &str) -> bool {
        let before = self.state.tools.len();
        self.state.tools.retain(|t| t.name() != name);
        self.state.tools.len() < before
    }

    /// Set the approval mode at runtime.
    pub const fn set_approval_mode(&mut self, mode: ApprovalMode) {
        self.approval_mode = mode;
    }

    // ── Tool Discovery ────────────────────────────────────────────────────

    /// Find a tool by name.
    #[must_use]
    pub fn find_tool(&self, name: &str) -> Option<&Arc<dyn AgentTool>> {
        self.state.tools.iter().find(|t| t.name() == name)
    }

    /// Return tools matching a predicate.
    #[must_use]
    pub fn tools_matching(
        &self,
        predicate: impl Fn(&dyn AgentTool) -> bool,
    ) -> Vec<&Arc<dyn AgentTool>> {
        self.state
            .tools
            .iter()
            .filter(|t| predicate(t.as_ref()))
            .collect()
    }

    /// Return tools belonging to the given namespace.
    ///
    /// Tools with no metadata or a different namespace are excluded.
    #[must_use]
    pub fn tools_in_namespace(&self, namespace: &str) -> Vec<&Arc<dyn AgentTool>> {
        self.state
            .tools
            .iter()
            .filter(|t| {
                t.metadata()
                    .and_then(|m| m.namespace)
                    .is_some_and(|ns| ns == namespace)
            })
            .collect()
    }

    /// Replace the entire message history.
    pub fn set_messages(&mut self, messages: Vec<AgentMessage>) {
        self.state.messages = messages;
    }

    /// Append messages to the history.
    pub fn append_messages(&mut self, messages: Vec<AgentMessage>) {
        self.state.messages.extend(messages);
    }

    /// Clear the message history.
    pub fn clear_messages(&mut self) {
        self.state.messages.clear();
    }

    // ── Checkpointing ────────────────────────────────────────────────────

    /// Create a checkpoint of the current agent state.
    ///
    /// If a [`CheckpointStore`] is configured, the checkpoint is also persisted.
    /// Returns the checkpoint regardless of whether a store is configured.
    pub async fn save_checkpoint(
        &self,
        id: impl Into<String>,
    ) -> Result<Checkpoint, std::io::Error> {
        let checkpoint = Checkpoint::new(
            id,
            &self.state.system_prompt,
            &self.state.model.provider,
            &self.state.model.model_id,
            &self.state.messages,
        );

        if let Some(ref store) = self.checkpoint_store {
            store.save_checkpoint(&checkpoint).await?;
        }

        Ok(checkpoint)
    }

    /// Restore agent message history from a checkpoint.
    ///
    /// Replaces the current messages with those from the checkpoint and
    /// updates the system prompt to match. Custom messages are not restored
    /// by this method; use `restore_messages(Some(registry))` directly for
    /// full restoration including custom messages.
    pub fn restore_from_checkpoint(&mut self, checkpoint: &Checkpoint) {
        self.state.messages = checkpoint.restore_messages(None);
        self.state
            .system_prompt
            .clone_from(&checkpoint.system_prompt);
    }

    /// Load a checkpoint from the configured store and restore state from it.
    ///
    /// Returns the loaded checkpoint, or `None` if not found.
    /// Returns an error if no checkpoint store is configured.
    pub async fn load_and_restore_checkpoint(
        &mut self,
        id: &str,
    ) -> Result<Option<Checkpoint>, std::io::Error> {
        let store = self
            .checkpoint_store
            .as_ref()
            .ok_or_else(|| std::io::Error::other("no checkpoint store configured"))?;

        let maybe = store.load_checkpoint(id).await?;
        if let Some(ref checkpoint) = maybe {
            self.restore_from_checkpoint(checkpoint);
        }
        Ok(maybe)
    }

    /// Access the checkpoint store, if configured.
    #[must_use]
    pub fn checkpoint_store(&self) -> Option<&dyn CheckpointStore> {
        self.checkpoint_store.as_deref()
    }

    // ── Plan Mode ───────────────────────────────────────────────────────

    /// Enter plan mode: restrict to read-only tools and append plan instructions.
    ///
    /// Saves the current tools and system prompt so they can be restored by
    /// [`exit_plan_mode`](Self::exit_plan_mode). Read-only tools are those where
    /// `requires_approval() == false`.
    pub fn enter_plan_mode(&mut self) -> (Vec<Arc<dyn AgentTool>>, String) {
        let state = &mut self.state;
        let saved_tools = state.tools.clone();
        let saved_prompt = state.system_prompt.clone();

        // Filter to read-only tools
        let read_only: Vec<Arc<dyn AgentTool>> = saved_tools
            .iter()
            .filter(|t| !t.requires_approval())
            .cloned()
            .collect();
        state.tools = read_only;

        // Append plan mode addendum
        let addendum = self
            .plan_mode_addendum
            .as_deref()
            .unwrap_or(DEFAULT_PLAN_MODE_ADDENDUM);
        state.system_prompt = format!("{}{addendum}", state.system_prompt);

        (saved_tools, saved_prompt)
    }

    /// Exit plan mode: restore previously saved tools and system prompt.
    pub fn exit_plan_mode(&mut self, saved_tools: Vec<Arc<dyn AgentTool>>, saved_prompt: String) {
        self.state.tools = saved_tools;
        self.state.system_prompt = saved_prompt;
    }

    // ── Queue Management ─────────────────────────────────────────────────

    /// Push a steering message into the queue.
    pub fn steer(&mut self, message: AgentMessage) {
        self.steering_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(message);
    }

    /// Push a follow-up message into the queue.
    pub fn follow_up(&mut self, message: AgentMessage) {
        self.follow_up_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(message);
    }

    /// Clear all steering messages.
    pub fn clear_steering(&mut self) {
        self.steering_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// Clear all follow-up messages.
    pub fn clear_follow_up(&mut self) {
        self.follow_up_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// Clear both steering and follow-up queues.
    pub fn clear_queues(&mut self) {
        self.clear_steering();
        self.clear_follow_up();
    }

    /// Returns `true` if there are pending steering or follow-up messages.
    #[must_use]
    pub fn has_pending_messages(&self) -> bool {
        let steering_empty = self
            .steering_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty();
        let follow_up_empty = self
            .follow_up_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty();
        !steering_empty || !follow_up_empty
    }

    // ── Control ──────────────────────────────────────────────────────────

    /// Cancel the currently running loop, if any.
    pub fn abort(&mut self) {
        if let Some(ref token) = self.abort_controller {
            info!("aborting agent loop");
            token.cancel();
        }
    }

    /// Pause the currently running loop and capture its state as a [`LoopCheckpoint`].
    ///
    /// Signals the loop to stop via the cancellation token and snapshots the
    /// agent's messages, system prompt, and model into a serializable
    /// checkpoint. The checkpoint can later be passed to [`resume`](Self::resume)
    /// to continue the loop from where it left off.
    ///
    /// Returns `None` if the agent is not currently running.
    pub fn pause(&mut self) -> Option<crate::checkpoint::LoopCheckpoint> {
        if !self.state.is_running {
            return None;
        }

        // Signal the loop to stop
        if let Some(ref token) = self.abort_controller {
            info!("pausing agent loop");
            token.cancel();
        }

        // Build the loop checkpoint from current state
        let checkpoint = crate::checkpoint::LoopCheckpoint::new(
            &self.state.system_prompt,
            &self.state.model.provider,
            &self.state.model.model_id,
            &self.state.messages,
        );

        self.state.is_running = false;
        self.abort_controller = None;
        self.idle_notify.notify_waiters();

        Some(checkpoint)
    }

    /// Resume the agent loop from a previously captured [`LoopCheckpoint`].
    ///
    /// Restores the message history, system prompt, and accumulated
    /// usage/cost from the checkpoint, then continues the loop via
    /// [`continue_async`](Self::continue_async).
    ///
    /// Any pending messages stored in the checkpoint are injected into the
    /// follow-up queue before resuming.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError::AlreadyRunning`] if the agent is already running,
    /// or [`AgentError::NoMessages`] if the checkpoint contains no messages.
    pub async fn resume(
        &mut self,
        checkpoint: &crate::checkpoint::LoopCheckpoint,
    ) -> Result<AgentResult, AgentError> {
        self.check_not_running()?;
        self.restore_from_loop_checkpoint(checkpoint)?;
        self.continue_async().await
    }

    /// Resume the agent loop from a checkpoint, returning an event stream.
    ///
    /// Streaming variant of [`resume`](Self::resume).
    ///
    /// # Errors
    ///
    /// Returns [`AgentError::AlreadyRunning`] if the agent is already running,
    /// or [`AgentError::NoMessages`] if the checkpoint contains no messages.
    pub fn resume_stream(
        &mut self,
        checkpoint: &crate::checkpoint::LoopCheckpoint,
    ) -> Result<Pin<Box<dyn Stream<Item = AgentEvent> + Send>>, AgentError> {
        self.check_not_running()?;
        self.restore_from_loop_checkpoint(checkpoint)?;
        self.continue_stream()
    }

    /// Internal helper: restore agent state from a loop checkpoint.
    fn restore_from_loop_checkpoint(
        &mut self,
        checkpoint: &crate::checkpoint::LoopCheckpoint,
    ) -> Result<(), AgentError> {
        self.state.messages = checkpoint.restore_messages(None);
        self.state
            .system_prompt
            .clone_from(&checkpoint.system_prompt);

        if self.state.messages.is_empty() {
            return Err(AgentError::NoMessages);
        }

        // Inject pending messages into the follow-up queue
        for msg in checkpoint.restore_pending_messages() {
            self.follow_up(msg);
        }

        info!(
            turn_index = checkpoint.turn_index,
            messages = self.state.messages.len(),
            "resuming agent loop from checkpoint"
        );

        Ok(())
    }

    /// Reset the agent to its initial state, clearing messages, queues, and error.
    pub fn reset(&mut self) {
        self.state.messages.clear();
        self.state.is_running = false;
        self.state.stream_message = None;
        self.state.pending_tool_calls.clear();
        self.state.error = None;
        self.abort_controller = None;
        self.in_flight_llm_messages = None;
        self.clear_queues();
    }

    /// Returns a future that resolves when the agent is no longer running.
    pub fn wait_for_idle(&self) -> impl Future<Output = ()> + Send + '_ {
        let notify = Arc::clone(&self.idle_notify);
        async move {
            // If already idle, return immediately.
            if !self.state.is_running {
                return;
            }
            notify.notified().await;
        }
    }

    // ── Observation ──────────────────────────────────────────────────────

    /// Subscribe to agent events. Returns a subscription ID for later removal.
    pub fn subscribe(
        &mut self,
        callback: impl Fn(&AgentEvent) + Send + Sync + 'static,
    ) -> SubscriptionId {
        self.listeners.subscribe(callback)
    }

    /// Remove a subscription. Returns `true` if the subscription existed.
    pub fn unsubscribe(&mut self, id: SubscriptionId) -> bool {
        self.listeners.unsubscribe(id)
    }

    /// Dispatch an event to all listeners, catching panics.
    ///
    /// Any listener that panics is automatically unsubscribed.
    fn dispatch_event(&mut self, event: &AgentEvent) {
        self.listeners.dispatch(event);

        // Forward to event forwarders
        for forwarder in &self.event_forwarders {
            forwarder(event.clone());
        }
    }

    /// Add an event forwarder at runtime.
    pub fn add_event_forwarder(&mut self, f: impl Fn(AgentEvent) + Send + Sync + 'static) {
        self.event_forwarders.push(Arc::new(f));
    }

    /// Dispatch an external event to all listeners and forwarders.
    ///
    /// Used for cross-agent event forwarding.
    pub fn forward_event(&mut self, event: &AgentEvent) {
        self.dispatch_event(event);
    }

    /// Emit a custom named event to all subscribers and forwarders.
    pub fn emit(&mut self, name: impl Into<String>, payload: serde_json::Value) {
        let event = AgentEvent::Custom(crate::emit::Emission::new(name, payload));
        self.dispatch_event(&event);
    }

    // ── Invocation: prompt ────────────────────────────────────────────────

    /// Start a new loop with input messages, returning an event stream.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError::AlreadyRunning`] if the agent is already running.
    pub fn prompt_stream(
        &mut self,
        input: Vec<AgentMessage>,
    ) -> Result<Pin<Box<dyn Stream<Item = AgentEvent> + Send>>, AgentError> {
        if let Err(e) = self.check_not_running() {
            warn!("prompt_stream called while agent is already running");
            return Err(e);
        }
        info!(
            model = %self.state.model.model_id,
            input_messages = input.len(),
            "prompt_stream starting"
        );
        self.start_loop(input, false)
    }

    /// Start a new loop with input messages, collecting to completion.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError::AlreadyRunning`] if the agent is already running.
    pub async fn prompt_async(
        &mut self,
        input: Vec<AgentMessage>,
    ) -> Result<AgentResult, AgentError> {
        info!(
            model = %self.state.model.model_id,
            input_messages = input.len(),
            "prompt_async starting"
        );
        let stream = self.prompt_stream(input)?;
        self.collect_stream(stream).await
    }

    /// Start a new loop with input messages, blocking the current thread.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError::AlreadyRunning`] if the agent is already running.
    pub fn prompt_sync(&mut self, input: Vec<AgentMessage>) -> Result<AgentResult, AgentError> {
        self.check_not_running()?;
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(async {
            let stream = self.start_loop(input, false)?;
            self.collect_stream(stream).await
        })
    }

    // ── Invocation: prompt (convenience) ────────────────────────────────

    /// Start a new loop from a plain text string, collecting to completion.
    ///
    /// Convenience wrapper that builds a `UserMessage` from the string.
    pub async fn prompt_text(
        &mut self,
        text: impl Into<String>,
    ) -> Result<AgentResult, AgentError> {
        let msg = AgentMessage::Llm(LlmMessage::User(crate::types::UserMessage {
            content: vec![ContentBlock::Text { text: text.into() }],
            timestamp: now_timestamp(),
        }));
        self.prompt_async(vec![msg]).await
    }

    /// Start a new loop from a text string with images, collecting to completion.
    ///
    /// Convenience wrapper that builds a `UserMessage` from text and image blocks.
    pub async fn prompt_text_with_images(
        &mut self,
        text: impl Into<String>,
        images: Vec<crate::types::ImageSource>,
    ) -> Result<AgentResult, AgentError> {
        let mut content = vec![ContentBlock::Text { text: text.into() }];
        for source in images {
            content.push(ContentBlock::Image { source });
        }
        let msg = AgentMessage::Llm(LlmMessage::User(crate::types::UserMessage {
            content,
            timestamp: now_timestamp(),
        }));
        self.prompt_async(vec![msg]).await
    }

    /// Start a new loop from a plain text string, blocking the current thread.
    ///
    /// Convenience wrapper that builds a `UserMessage` from the string.
    pub fn prompt_text_sync(&mut self, text: impl Into<String>) -> Result<AgentResult, AgentError> {
        let msg = AgentMessage::Llm(LlmMessage::User(crate::types::UserMessage {
            content: vec![ContentBlock::Text { text: text.into() }],
            timestamp: now_timestamp(),
        }));
        self.prompt_sync(vec![msg])
    }

    // ── Invocation: continue ─────────────────────────────────────────────

    /// Continue from existing messages, returning an event stream.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError::AlreadyRunning`], [`AgentError::NoMessages`],
    /// or [`AgentError::InvalidContinue`].
    pub fn continue_stream(
        &mut self,
    ) -> Result<Pin<Box<dyn Stream<Item = AgentEvent> + Send>>, AgentError> {
        self.check_not_running()?;
        self.validate_continue()?;
        self.start_loop(Vec::new(), true)
    }

    /// Continue from existing messages, collecting to completion.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError::AlreadyRunning`], [`AgentError::NoMessages`],
    /// or [`AgentError::InvalidContinue`].
    pub async fn continue_async(&mut self) -> Result<AgentResult, AgentError> {
        let stream = self.continue_stream()?;
        self.collect_stream(stream).await
    }

    /// Continue from existing messages, blocking the current thread.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError::AlreadyRunning`], [`AgentError::NoMessages`],
    /// or [`AgentError::InvalidContinue`].
    pub fn continue_sync(&mut self) -> Result<AgentResult, AgentError> {
        self.check_not_running()?;
        self.validate_continue()?;
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(async {
            let stream = self.start_loop(Vec::new(), true)?;
            self.collect_stream(stream).await
        })
    }

    // ── Structured Output ────────────────────────────────────────────────

    /// Run a structured output extraction loop.
    ///
    /// Injects a synthetic `__structured_output` tool, runs the loop, extracts
    /// the tool call arguments, validates against the schema, and retries on
    /// failure up to `structured_output_max_retries`.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError::StructuredOutputFailed`] if validation fails
    /// after all retries, or any error from the underlying loop.
    pub async fn structured_output(
        &mut self,
        prompt: String,
        schema: Value,
    ) -> Result<Value, AgentError> {
        let tool = Arc::new(StructuredOutputTool {
            schema: schema.clone(),
        });

        // Temporarily add the synthetic tool
        self.state.tools.push(tool);

        let mut last_error = String::new();
        let max_retries = self.structured_output_max_retries;

        for attempt in 0..=max_retries {
            let result = if attempt == 0 {
                let user_msg = AgentMessage::Llm(LlmMessage::User(crate::types::UserMessage {
                    content: vec![ContentBlock::Text {
                        text: prompt.clone(),
                    }],
                    timestamp: now_timestamp(),
                }));
                self.prompt_async(vec![user_msg]).await?
            } else {
                self.continue_async().await?
            };

            match extract_structured_output(&result, &schema) {
                Ok(value) => {
                    self.remove_structured_output_tool();
                    return Ok(value);
                }
                Err(e) => {
                    last_error.clone_from(&e);
                    if attempt < max_retries {
                        // Inject error feedback for retry
                        let feedback = AgentMessage::Llm(LlmMessage::ToolResult(
                            crate::types::ToolResultMessage {
                                tool_call_id: find_structured_output_call_id(&result)
                                    .unwrap_or_default(),
                                content: vec![ContentBlock::Text {
                                    text: format!(
                                        "Validation failed: {e}. Please try again with valid \
                                         output."
                                    ),
                                }],
                                is_error: true,
                                timestamp: now_timestamp(),
                                details: serde_json::Value::Null,
                            },
                        ));
                        self.state.messages.push(feedback);
                    }
                }
            }
        }

        self.remove_structured_output_tool();
        Err(AgentError::StructuredOutputFailed {
            attempts: max_retries + 1,
            last_error,
        })
    }

    /// Run a structured output extraction loop, blocking the current thread.
    ///
    /// Sync variant of [`structured_output`](Self::structured_output).
    pub fn structured_output_sync(
        &mut self,
        prompt: String,
        schema: Value,
    ) -> Result<Value, AgentError> {
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(self.structured_output(prompt, schema))
    }

    /// Run structured output extraction and deserialize into a typed result.
    ///
    /// Validates against the schema, then deserializes the `Value` into `T`.
    pub async fn structured_output_typed<T: serde::de::DeserializeOwned>(
        &mut self,
        prompt: String,
        schema: Value,
    ) -> Result<T, AgentError> {
        let value = self.structured_output(prompt, schema).await?;
        serde_json::from_value(value).map_err(|e| AgentError::StructuredOutputFailed {
            attempts: 1,
            last_error: format!("deserialization failed: {e}"),
        })
    }

    /// Run structured output extraction, deserialize into a typed result, blocking.
    ///
    /// Sync variant of [`structured_output_typed`](Self::structured_output_typed).
    pub fn structured_output_typed_sync<T: serde::de::DeserializeOwned>(
        &mut self,
        prompt: String,
        schema: Value,
    ) -> Result<T, AgentError> {
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(self.structured_output_typed(prompt, schema))
    }

    // ── Private Helpers ──────────────────────────────────────────────────

    const fn check_not_running(&self) -> Result<(), AgentError> {
        if self.state.is_running {
            return Err(AgentError::AlreadyRunning);
        }
        Ok(())
    }

    fn validate_continue(&self) -> Result<(), AgentError> {
        if self.state.messages.is_empty() {
            return Err(AgentError::NoMessages);
        }
        if let Some(AgentMessage::Llm(LlmMessage::Assistant(_))) = self.state.messages.last()
            && !self.has_pending_messages()
        {
            return Err(AgentError::InvalidContinue);
        }
        Ok(())
    }

    fn remove_structured_output_tool(&mut self) {
        self.state
            .tools
            .retain(|t| t.name() != "__structured_output");
    }

    /// Build the loop config and start the agent loop, returning a wrapped stream.
    #[allow(clippy::unnecessary_wraps)]
    fn start_loop(
        &mut self,
        input: Vec<AgentMessage>,
        is_continue: bool,
    ) -> Result<Pin<Box<dyn Stream<Item = AgentEvent> + Send>>, AgentError> {
        self.state.is_running = true;
        self.state.error = None;

        let token = CancellationToken::new();
        self.abort_controller = Some(token.clone());

        let config = self.build_loop_config();
        let system_prompt = self.state.system_prompt.clone();
        let in_flight_llm_messages = if is_continue {
            self.state
                .messages
                .iter()
                .filter_map(|msg| match msg {
                    AgentMessage::Llm(llm) => Some(AgentMessage::Llm(llm.clone())),
                    AgentMessage::Custom(_) => None,
                })
                .collect()
        } else {
            self.state
                .messages
                .iter()
                .chain(input.iter())
                .filter_map(|msg| match msg {
                    AgentMessage::Llm(llm) => Some(AgentMessage::Llm(llm.clone())),
                    AgentMessage::Custom(_) => None,
                })
                .collect()
        };

        let messages_for_loop = if is_continue {
            std::mem::take(&mut self.state.messages)
        } else {
            let mut msgs = std::mem::take(&mut self.state.messages);
            msgs.extend(input);
            msgs
        };

        let raw_stream = if is_continue {
            agent_loop_continue(messages_for_loop, system_prompt, config, token)
        } else {
            agent_loop(messages_for_loop, system_prompt, config, token)
        };

        self.in_flight_llm_messages = Some(in_flight_llm_messages);

        Ok(raw_stream)
    }

    #[allow(clippy::type_complexity)]
    fn build_loop_config(&self) -> AgentLoopConfig {
        // Clone Arcs to share closures with the loop config
        let convert = Arc::clone(&self.convert_to_llm);
        let convert_box: Box<dyn Fn(&AgentMessage) -> Option<LlmMessage> + Send + Sync> =
            Box::new(move |msg| convert(msg));

        let transform = self.transform_context.as_ref().map(Arc::clone);

        let api_key_box = self.get_api_key.as_ref().map(|k| {
            let k = Arc::clone(k);
            let b: Box<
                dyn Fn(&str) -> Pin<Box<dyn Future<Output = Option<String>> + Send>> + Send + Sync,
            > = Box::new(move |provider| k(provider));
            b
        });

        let queue_provider: Arc<dyn MessageProvider> = Arc::new(QueueMessageProvider {
            steering_queue: Arc::clone(&self.steering_queue),
            follow_up_queue: Arc::clone(&self.follow_up_queue),
            steering_mode: self.steering_mode,
            follow_up_mode: self.follow_up_mode,
        });

        let message_provider: Arc<dyn MessageProvider> =
            if let Some(ref external) = self.external_message_provider {
                Arc::new(crate::message_provider::ComposedMessageProvider::new(
                    queue_provider,
                    Arc::clone(external),
                ))
            } else {
                queue_provider
            };

        AgentLoopConfig {
            model: self.state.model.clone(),
            stream_options: self.stream_options.clone(),
            retry_strategy: Box::new(SharedRetryStrategy(Arc::clone(&self.retry_strategy))),
            stream_fn: Arc::clone(&self.stream_fn),
            tools: self.state.tools.clone(),
            convert_to_llm: convert_box,
            transform_context: transform,
            get_api_key: api_key_box,
            message_provider: Some(message_provider),
            approve_tool: self.approve_tool.as_ref().map(|a| {
                let a = Arc::clone(a);
                let b: Box<ApproveToolFn> = Box::new(move |req| a(req));
                b
            }),
            approval_mode: self.approval_mode,
            tool_validator: self.tool_validator.clone(),
            loop_policy: self.loop_policy.clone(),
            tool_call_transformer: self.tool_call_transformer.clone(),
            post_turn_hook: self.post_turn_hook.clone(),
            async_transform_context: self.async_transform_context.as_ref().map(Arc::clone),
            metrics_collector: self.metrics_collector.as_ref().map(Arc::clone),
            fallback: self.fallback.clone(),
            budget_guard: self.budget_guard.clone(),
            tool_execution_policy: self.tool_execution_policy.clone(),
        }
    }

    /// Collect a stream to completion, updating agent state along the way.
    async fn collect_stream(
        &mut self,
        mut stream: Pin<Box<dyn Stream<Item = AgentEvent> + Send>>,
    ) -> Result<AgentResult, AgentError> {
        let mut all_messages: Vec<AgentMessage> = Vec::new();
        let mut state_messages = self.in_flight_llm_messages.take().unwrap_or_default();
        let mut received_full_context = false;
        let mut stop_reason = StopReason::Stop;
        let mut usage = Usage::default();
        let mut cost = Cost::default();
        let mut error: Option<String> = None;

        while let Some(event) = stream.next().await {
            self.dispatch_event(&event);
            self.update_state_from_event(&event);

            match event {
                AgentEvent::TurnEnd {
                    assistant_message,
                    tool_results,
                    ..
                } => {
                    stop_reason = assistant_message.stop_reason;
                    usage += assistant_message.usage.clone();
                    cost += assistant_message.cost.clone();
                    if let Some(ref err) = assistant_message.error_message {
                        error = Some(err.clone());
                    }
                    let assistant_msg = AgentMessage::Llm(LlmMessage::Assistant(assistant_message));
                    state_messages.push(match &assistant_msg {
                        AgentMessage::Llm(msg) => AgentMessage::Llm(msg.clone()),
                        AgentMessage::Custom(_) => unreachable!(),
                    });
                    all_messages.push(assistant_msg);
                    for tr in tool_results {
                        state_messages.push(AgentMessage::Llm(LlmMessage::ToolResult(tr.clone())));
                        all_messages.push(AgentMessage::Llm(LlmMessage::ToolResult(tr)));
                    }
                }
                AgentEvent::AgentEnd { messages } => {
                    if let Ok(returned) = Arc::try_unwrap(messages) {
                        self.state.messages = returned;
                        received_full_context = true;
                    }
                }
                _ => {}
            }
        }

        if !received_full_context {
            self.state.messages = state_messages;
        }
        self.state.is_running = false;
        self.state.error.clone_from(&error);
        self.idle_notify.notify_waiters();

        Ok(AgentResult {
            messages: all_messages,
            stop_reason,
            usage,
            cost,
            error,
        })
    }

    /// Processes a streaming event, updating [`Agent::state`] and notifying subscribers.
    ///
    /// **Required** when using [`Agent::prompt_stream`]. Call this for every event received
    /// on the channel, or `agent.state()` will not reflect the current run status.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut rx = agent.prompt_stream(messages).await?;
    /// while let Some(event) = rx.recv().await {
    ///     agent.handle_stream_event(&event);
    ///     // render event to UI...
    /// }
    /// ```
    pub fn handle_stream_event(&mut self, event: &AgentEvent) {
        self.dispatch_event(event);
        self.update_state_from_event(event);

        match event {
            AgentEvent::TurnEnd {
                assistant_message,
                tool_results,
                ..
            } => {
                // Accumulate messages into in-flight state, mirroring collect_stream.
                let msgs = self.in_flight_llm_messages.get_or_insert_with(Vec::new);
                msgs.push(AgentMessage::Llm(LlmMessage::Assistant(
                    assistant_message.clone(),
                )));
                for tr in tool_results {
                    msgs.push(AgentMessage::Llm(LlmMessage::ToolResult(tr.clone())));
                }
            }
            AgentEvent::AgentEnd { messages } => {
                // The loop returns the full context. Try to take ownership;
                // fall back to the accumulated in-flight messages.
                if let Ok(returned) = Arc::try_unwrap(messages.clone()) {
                    self.state.messages = returned;
                } else if let Some(msgs) = self.in_flight_llm_messages.take() {
                    self.state.messages = msgs;
                }
                self.state.error = None;
                self.idle_notify.notify_waiters();
            }
            _ => {}
        }
    }

    /// Update internal tracking state from an event.
    fn update_state_from_event(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::MessageStart => {
                self.state.stream_message = None;
            }
            AgentEvent::MessageEnd { message } => {
                self.state.stream_message =
                    Some(AgentMessage::Llm(LlmMessage::Assistant(message.clone())));
            }
            AgentEvent::ToolExecutionStart { id, .. } => {
                self.state.pending_tool_calls.insert(id.clone());
            }
            AgentEvent::ToolExecutionEnd { result, .. } => {
                // We don't have the ID directly on ToolExecutionEnd, so we
                // clear pending_tool_calls when the turn ends.
                let _ = result;
            }
            AgentEvent::TurnEnd { .. } => {
                self.state.pending_tool_calls.clear();
                self.state.stream_message = None;
            }
            AgentEvent::AgentEnd { .. } => {
                self.state.is_running = false;
                self.state.pending_tool_calls.clear();
                self.state.stream_message = None;
            }
            _ => {}
        }
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

// ─── QueueMessageProvider ────────────────────────────────────────────────────

/// [`MessageProvider`] backed by shared steering and follow-up queues.
///
/// Drains messages according to the configured [`SteeringMode`] and
/// [`FollowUpMode`] — either one at a time or all at once.
struct QueueMessageProvider {
    steering_queue: Arc<Mutex<Vec<AgentMessage>>>,
    follow_up_queue: Arc<Mutex<Vec<AgentMessage>>>,
    steering_mode: SteeringMode,
    follow_up_mode: FollowUpMode,
}

impl MessageProvider for QueueMessageProvider {
    fn poll_steering(&self) -> Vec<AgentMessage> {
        drain_queue(
            &self.steering_queue,
            self.steering_mode == SteeringMode::OneAtATime,
        )
    }

    fn poll_follow_up(&self) -> Vec<AgentMessage> {
        drain_queue(
            &self.follow_up_queue,
            self.follow_up_mode == FollowUpMode::OneAtATime,
        )
    }
}

// ─── Queue Draining ──────────────────────────────────────────────────────────

fn drain_queue(queue: &Mutex<Vec<AgentMessage>>, one_at_a_time: bool) -> Vec<AgentMessage> {
    let mut guard = queue
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if guard.is_empty() {
        return Vec::new();
    }
    if one_at_a_time {
        vec![guard.remove(0)]
    } else {
        std::mem::take(&mut *guard)
    }
}

// ─── Structured Output Helpers ───────────────────────────────────────────────

/// Synthetic tool used for structured output extraction.
struct StructuredOutputTool {
    schema: Value,
}

#[allow(clippy::unnecessary_literal_bound)]
impl crate::tool::AgentTool for StructuredOutputTool {
    fn name(&self) -> &str {
        "__structured_output"
    }

    fn label(&self) -> &str {
        "Structured Output"
    }

    fn description(&self) -> &str {
        "Return structured data matching the required JSON schema. Call this tool with the \
         requested data as the arguments."
    }

    fn parameters_schema(&self) -> &Value {
        &self.schema
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(crate::tool::AgentToolResult) + Send + Sync>>,
    ) -> Pin<Box<dyn Future<Output = crate::tool::AgentToolResult> + Send + '_>> {
        Box::pin(async move {
            crate::tool::AgentToolResult::text(serde_json::to_string(&params).unwrap_or_default())
        })
    }
}

/// Extract structured output from an agent result by finding the
/// `__structured_output` tool call and validating its arguments.
fn extract_structured_output(result: &AgentResult, schema: &Value) -> Result<Value, String> {
    for msg in &result.messages {
        if let AgentMessage::Llm(LlmMessage::Assistant(assistant)) = msg {
            for block in &assistant.content {
                if let ContentBlock::ToolCall {
                    name, arguments, ..
                } = block
                    && name == "__structured_output"
                {
                    // Validate against schema
                    let validation = crate::tool::validate_tool_arguments(schema, arguments);
                    match validation {
                        Ok(()) => return Ok(arguments.clone()),
                        Err(errors) => {
                            return Err(errors.join("; "));
                        }
                    }
                }
            }
        }
    }
    Err("no __structured_output tool call found in response".to_string())
}

/// Find the tool call ID of the `__structured_output` call in an agent result.
fn find_structured_output_call_id(result: &AgentResult) -> Option<String> {
    for msg in &result.messages {
        if let AgentMessage::Llm(LlmMessage::Assistant(assistant)) = msg {
            for block in &assistant.content {
                if let ContentBlock::ToolCall { name, id, .. } = block
                    && name == "__structured_output"
                {
                    return Some(id.clone());
                }
            }
        }
    }
    None
}
