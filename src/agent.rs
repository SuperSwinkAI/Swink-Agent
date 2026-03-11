//! Stateful public API wrapper over the agent loop.
//!
//! The [`Agent`] struct owns conversation state, configuration, and queue
//! management. It provides three invocation modes (`prompt_stream`,
//! `prompt_async`, `prompt_sync`), structured output extraction, steering and
//! follow-up queues, and an observer/subscriber pattern for event dispatch.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures::{Stream, StreamExt};
use serde_json::Value;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::error::HarnessError;
use crate::loop_::{AgentEvent, AgentLoopConfig, agent_loop, agent_loop_continue};
use crate::retry::{DefaultRetryStrategy, RetryStrategy};
use crate::stream::{StreamFn, StreamOptions};
use crate::tool::AgentTool;
use crate::types::{
    AgentMessage, AgentResult, ContentBlock, Cost, LlmMessage, ModelSpec, StopReason,
    ThinkingLevel, Usage,
};

// ─── SubscriptionId ──────────────────────────────────────────────────────────

/// Unique identifier for an event subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionId(u64);

impl SubscriptionId {
    fn next() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

// ─── Enums ───────────────────────────────────────────────────────────────────

/// Controls how steering messages are drained from the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SteeringMode {
    /// Drain all pending steering messages at once.
    All,
    /// Drain one steering message per poll.
    #[default]
    OneAtATime,
}

/// Controls how follow-up messages are drained from the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FollowUpMode {
    /// Drain all pending follow-up messages at once.
    All,
    /// Drain one follow-up message per poll.
    #[default]
    OneAtATime,
}

// ─── Type Aliases ────────────────────────────────────────────────────────────

type ConvertToLlmFn = Arc<dyn Fn(&AgentMessage) -> Option<LlmMessage> + Send + Sync>;
type TransformContextFn = Arc<dyn Fn(&mut Vec<AgentMessage>, bool) + Send + Sync>;
type GetApiKeyFn =
    Arc<dyn Fn(&str) -> Pin<Box<dyn Future<Output = Option<String>> + Send>> + Send + Sync>;
type ListenerFn = Box<dyn Fn(&AgentEvent) + Send + Sync>;

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
            .finish()
    }
}

// ─── AgentOptions ────────────────────────────────────────────────────────────

/// Configuration options for constructing an [`Agent`].
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
    /// Optional context transformation hook.
    pub transform_context: Option<TransformContextFn>,
    /// Optional async API key resolver.
    pub get_api_key: Option<GetApiKeyFn>,
    /// Retry strategy for transient failures.
    pub retry_strategy: Box<dyn RetryStrategy>,
    /// Per-call stream options.
    pub stream_options: StreamOptions,
    /// Steering queue drain mode.
    pub steering_mode: SteeringMode,
    /// Follow-up queue drain mode.
    pub follow_up_mode: FollowUpMode,
    /// Max retries for structured output validation.
    pub structured_output_max_retries: usize,
}

impl AgentOptions {
    /// Create options with required fields and sensible defaults.
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
            transform_context: Some(Arc::new(crate::context::sliding_window(
                100_000, 50_000, 2,
            ))),
            get_api_key: None,
            retry_strategy: Box::new(DefaultRetryStrategy::default()),
            stream_options: StreamOptions::default(),
            steering_mode: SteeringMode::default(),
            follow_up_mode: FollowUpMode::default(),
            structured_output_max_retries: 3,
        }
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

    /// Set the context transform hook.
    #[must_use]
    pub fn with_transform_context(
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
    pub const fn with_steering_mode(mut self, mode: SteeringMode) -> Self {
        self.steering_mode = mode;
        self
    }

    /// Set the follow-up mode.
    #[must_use]
    pub const fn with_follow_up_mode(mut self, mode: FollowUpMode) -> Self {
        self.follow_up_mode = mode;
        self
    }

    /// Set the max retries for structured output.
    #[must_use]
    pub const fn with_structured_output_max_retries(mut self, n: usize) -> Self {
        self.structured_output_max_retries = n;
        self
    }
}

// ─── Agent ───────────────────────────────────────────────────────────────────

/// Stateful wrapper over the agent loop.
///
/// Owns conversation history, configuration, steering/follow-up queues, and
/// subscriber callbacks. Provides prompt, continue, and structured output
/// invocation modes.
pub struct Agent {
    // ── Public-facing state ──
    state: AgentState,

    // ── Private fields ──
    steering_queue: Arc<Mutex<Vec<AgentMessage>>>,
    follow_up_queue: Arc<Mutex<Vec<AgentMessage>>>,
    listeners: HashMap<SubscriptionId, ListenerFn>,
    abort_controller: Option<CancellationToken>,
    steering_mode: SteeringMode,
    follow_up_mode: FollowUpMode,
    stream_fn: Arc<dyn StreamFn>,
    convert_to_llm: ConvertToLlmFn,
    transform_context: Option<TransformContextFn>,
    get_api_key: Option<GetApiKeyFn>,
    retry_strategy: Arc<dyn RetryStrategy>,
    stream_options: StreamOptions,
    structured_output_max_retries: usize,
    idle_notify: Arc<Notify>,
}

impl Agent {
    /// Create a new agent from the given options.
    pub fn new(options: AgentOptions) -> Self {
        Self {
            state: AgentState {
                system_prompt: options.system_prompt,
                model: options.model,
                tools: options.tools,
                messages: Vec::new(),
                is_running: false,
                stream_message: None,
                pending_tool_calls: HashSet::new(),
                error: None,
            },
            steering_queue: Arc::new(Mutex::new(Vec::new())),
            follow_up_queue: Arc::new(Mutex::new(Vec::new())),
            listeners: HashMap::new(),
            abort_controller: None,
            steering_mode: options.steering_mode,
            follow_up_mode: options.follow_up_mode,
            stream_fn: options.stream_fn,
            convert_to_llm: options.convert_to_llm,
            transform_context: options.transform_context,
            get_api_key: options.get_api_key,
            retry_strategy: Arc::from(options.retry_strategy),
            stream_options: options.stream_options,
            structured_output_max_retries: options.structured_output_max_retries,
            idle_notify: Arc::new(Notify::new()),
        }
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

    /// Set the model specification.
    pub fn set_model(&mut self, model: ModelSpec) {
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

    /// Reset the agent to its initial state, clearing messages, queues, and error.
    pub fn reset(&mut self) {
        self.state.messages.clear();
        self.state.is_running = false;
        self.state.stream_message = None;
        self.state.pending_tool_calls.clear();
        self.state.error = None;
        self.abort_controller = None;
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
        let id = SubscriptionId::next();
        self.listeners.insert(id, Box::new(callback));
        id
    }

    /// Remove a subscription. Returns `true` if the subscription existed.
    pub fn unsubscribe(&mut self, id: SubscriptionId) -> bool {
        self.listeners.remove(&id).is_some()
    }

    /// Dispatch an event to all listeners, catching panics.
    fn dispatch_event(&self, event: &AgentEvent) {
        for listener in self.listeners.values() {
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                listener(event);
            }));
            if let Err(e) = result {
                eprintln!("listener panic: {e:?}");
            }
        }
    }

    // ── Invocation: prompt ────────────────────────────────────────────────

    /// Start a new loop with input messages, returning an event stream.
    ///
    /// # Errors
    ///
    /// Returns [`HarnessError::AlreadyRunning`] if the agent is already running.
    pub fn prompt_stream(
        &mut self,
        input: Vec<AgentMessage>,
    ) -> Result<Pin<Box<dyn Stream<Item = AgentEvent> + Send>>, HarnessError> {
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
    /// Returns [`HarnessError::AlreadyRunning`] if the agent is already running.
    pub async fn prompt_async(
        &mut self,
        input: Vec<AgentMessage>,
    ) -> Result<AgentResult, HarnessError> {
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
    /// Returns [`HarnessError::AlreadyRunning`] if the agent is already running.
    pub fn prompt_sync(&mut self, input: Vec<AgentMessage>) -> Result<AgentResult, HarnessError> {
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
    pub async fn prompt_text(&mut self, text: impl Into<String>) -> Result<AgentResult, HarnessError> {
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
    ) -> Result<AgentResult, HarnessError> {
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
    pub fn prompt_text_sync(&mut self, text: impl Into<String>) -> Result<AgentResult, HarnessError> {
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
    /// Returns [`HarnessError::AlreadyRunning`], [`HarnessError::NoMessages`],
    /// or [`HarnessError::InvalidContinue`].
    pub fn continue_stream(
        &mut self,
    ) -> Result<Pin<Box<dyn Stream<Item = AgentEvent> + Send>>, HarnessError> {
        self.check_not_running()?;
        self.validate_continue()?;
        self.start_loop(Vec::new(), true)
    }

    /// Continue from existing messages, collecting to completion.
    ///
    /// # Errors
    ///
    /// Returns [`HarnessError::AlreadyRunning`], [`HarnessError::NoMessages`],
    /// or [`HarnessError::InvalidContinue`].
    pub async fn continue_async(&mut self) -> Result<AgentResult, HarnessError> {
        let stream = self.continue_stream()?;
        self.collect_stream(stream).await
    }

    /// Continue from existing messages, blocking the current thread.
    ///
    /// # Errors
    ///
    /// Returns [`HarnessError::AlreadyRunning`], [`HarnessError::NoMessages`],
    /// or [`HarnessError::InvalidContinue`].
    pub fn continue_sync(&mut self) -> Result<AgentResult, HarnessError> {
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
    /// Returns [`HarnessError::StructuredOutputFailed`] if validation fails
    /// after all retries, or any error from the underlying loop.
    pub async fn structured_output(
        &mut self,
        prompt: String,
        schema: Value,
    ) -> Result<Value, HarnessError> {
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
                                        "Validation failed: {e}. Please try again with valid output."
                                    ),
                                }],
                                is_error: true,
                                timestamp: now_timestamp(),
                            },
                        ));
                        self.state.messages.push(feedback);
                    }
                }
            }
        }

        self.remove_structured_output_tool();
        Err(HarnessError::StructuredOutputFailed {
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
    ) -> Result<Value, HarnessError> {
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
    ) -> Result<T, HarnessError> {
        let value = self.structured_output(prompt, schema).await?;
        serde_json::from_value(value).map_err(|e| HarnessError::StructuredOutputFailed {
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
    ) -> Result<T, HarnessError> {
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(self.structured_output_typed(prompt, schema))
    }

    // ── Private Helpers ──────────────────────────────────────────────────

    const fn check_not_running(&self) -> Result<(), HarnessError> {
        if self.state.is_running {
            return Err(HarnessError::AlreadyRunning);
        }
        Ok(())
    }

    fn validate_continue(&self) -> Result<(), HarnessError> {
        if self.state.messages.is_empty() {
            return Err(HarnessError::NoMessages);
        }
        if let Some(AgentMessage::Llm(LlmMessage::Assistant(_))) = self.state.messages.last() {
            if !self.has_pending_messages() {
                return Err(HarnessError::InvalidContinue);
            }
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
    ) -> Result<Pin<Box<dyn Stream<Item = AgentEvent> + Send>>, HarnessError> {
        self.state.is_running = true;
        self.state.error = None;

        let token = CancellationToken::new();
        self.abort_controller = Some(token.clone());

        let config = self.build_loop_config();
        let system_prompt = self.state.system_prompt.clone();

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

        Ok(raw_stream)
    }

    #[allow(clippy::type_complexity)]
    fn build_loop_config(&self) -> AgentLoopConfig {
        let steering_queue = Arc::clone(&self.steering_queue);
        let steering_mode = self.steering_mode;

        let follow_up_queue = Arc::clone(&self.follow_up_queue);
        let follow_up_mode = self.follow_up_mode;

        // Clone Arcs to share closures with the loop config
        let convert = Arc::clone(&self.convert_to_llm);
        let convert_box: Box<dyn Fn(&AgentMessage) -> Option<LlmMessage> + Send + Sync> =
            Box::new(move |msg| convert(msg));

        let transform_box = self.transform_context.as_ref().map(|t| {
            let t = Arc::clone(t);
            let b: Box<dyn Fn(&mut Vec<AgentMessage>, bool) + Send + Sync> =
                Box::new(move |msgs, overflow| t(msgs, overflow));
            b
        });

        let api_key_box = self.get_api_key.as_ref().map(|k| {
            let k = Arc::clone(k);
            let b: Box<
                dyn Fn(&str) -> Pin<Box<dyn Future<Output = Option<String>> + Send>> + Send + Sync,
            > = Box::new(move |provider| k(provider));
            b
        });

        AgentLoopConfig {
            model: self.state.model.clone(),
            stream_options: self.stream_options.clone(),
            retry_strategy: Box::new(SharedRetryStrategy(Arc::clone(&self.retry_strategy))),
            stream_fn: Arc::clone(&self.stream_fn),
            tools: self.state.tools.clone(),
            convert_to_llm: convert_box,
            transform_context: transform_box,
            get_api_key: api_key_box,
            get_steering_messages: Some(Box::new(move || {
                drain_queue(&steering_queue, steering_mode == SteeringMode::OneAtATime)
            })),
            get_follow_up_messages: Some(Box::new(move || {
                drain_queue(&follow_up_queue, follow_up_mode == FollowUpMode::OneAtATime)
            })),
        }
    }

    /// Collect a stream to completion, updating agent state along the way.
    async fn collect_stream(
        &mut self,
        mut stream: Pin<Box<dyn Stream<Item = AgentEvent> + Send>>,
    ) -> Result<AgentResult, HarnessError> {
        let mut all_messages: Vec<AgentMessage> = Vec::new();
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
                } => {
                    stop_reason = assistant_message.stop_reason;
                    usage += assistant_message.usage;
                    cost += assistant_message.cost;
                    if let Some(ref err) = assistant_message.error_message {
                        error = Some(err.clone());
                    }
                    all_messages.push(AgentMessage::Llm(LlmMessage::Assistant(assistant_message)));
                    for tr in tool_results {
                        all_messages.push(AgentMessage::Llm(LlmMessage::ToolResult(tr)));
                    }
                }
                AgentEvent::AgentEnd { messages } => {
                    // Merge the returned messages into our state.
                    // The loop returns all messages it produced; we take those
                    // as the canonical set. We consume the Arc if possible,
                    // otherwise we cannot clone AgentMessage (custom messages
                    // are not cloneable), so we just note the count.
                    if let Ok(returned) = Arc::try_unwrap(messages) {
                        self.state.messages.extend(returned);
                    }
                    // If Arc::try_unwrap fails, the messages are already
                    // captured in `all_messages` from TurnEnd events above.
                }
                _ => {}
            }
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
    fn should_retry(&self, error: &HarnessError, attempt: u32) -> bool {
        self.0.should_retry(error, attempt)
    }

    fn delay(&self, attempt: u32) -> std::time::Duration {
        self.0.delay(attempt)
    }
}

// ─── Queue Draining ──────────────────────────────────────────────────────────

fn drain_queue(queue: &Mutex<Vec<AgentMessage>>, one_at_a_time: bool) -> Vec<AgentMessage> {
    let mut guard = queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
impl AgentTool for StructuredOutputTool {
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
                {
                    if name == "__structured_output" {
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
    }
    Err("no __structured_output tool call found in response".to_string())
}

/// Find the tool call ID of the `__structured_output` call in an agent result.
fn find_structured_output_call_id(result: &AgentResult) -> Option<String> {
    for msg in &result.messages {
        if let AgentMessage::Llm(LlmMessage::Assistant(assistant)) = msg {
            for block in &assistant.content {
                if let ContentBlock::ToolCall { name, id, .. } = block {
                    if name == "__structured_output" {
                        return Some(id.clone());
                    }
                }
            }
        }
    }
    None
}

/// Get the current Unix timestamp in seconds.
fn now_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
