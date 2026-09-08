use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::{Context, Poll};

use futures::Stream;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::agent_options::{ApproveToolFn, GetApiKeyFn};
use crate::error::AgentError;
use crate::loop_::{
    AgentEvent, AgentLoopConfig, agent_loop_continue, agent_loop_with_initial_new_messages_len,
};
use crate::message_provider::MessageProvider;
use crate::types::message_codec::clone_messages_for_send;
use crate::types::{AgentMessage, AgentResult, ContentBlock, LlmMessage};
use crate::util::now_timestamp;

use super::queueing::{QueueMessageProvider, drain_messages_from_queue};
use super::{Agent, SharedRetryStrategy};

// ─── LoopGuardStream ────────────────────────────────────────────────────────

/// Wrapper stream that clears the agent's `loop_active` flag when dropped.
///
/// This ensures the agent becomes idle even if the caller drops the stream
/// without draining it to `AgentEnd`. A generation counter prevents a stale
/// guard from clearing the flag for a newer run.
///
/// Note on history preservation: this guard deliberately does **not** touch
/// `state.messages`. `start_loop` keeps a snapshot of the full pre-run
/// context in `state.messages` (rather than leaving it empty after moving
/// the history into the loop task), so an early drop has nothing to restore
/// — the history was never removed from observable state. Attempting a
/// restore from `Drop` would also be unsound: the guard has no access to
/// `&mut Agent`, and the spawned loop task may still be running when `Drop`
/// executes (cancellation is signalled here, not joined).
struct LoopGuardStream {
    inner: Pin<Box<dyn Stream<Item = AgentEvent> + Send>>,
    cancellation_token: CancellationToken,
    loop_active: Arc<AtomicBool>,
    idle_notify: Arc<Notify>,
    pending_message_snapshot: Arc<crate::pause_state::PendingMessageSnapshot>,
    loop_context_snapshot: Arc<crate::pause_state::LoopContextSnapshot>,
    generation: u64,
    expected_generation: Arc<AtomicU64>,
}

impl Stream for LoopGuardStream {
    type Item = AgentEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl Drop for LoopGuardStream {
    fn drop(&mut self) {
        // Only clear loop_active if this guard belongs to the current run.
        // A newer start_loop will have incremented loop_generation, making
        // this guard's generation stale.
        if self.expected_generation.load(Ordering::Acquire) == self.generation {
            self.cancellation_token.cancel();
            self.loop_active.store(false, Ordering::Release);
            self.pending_message_snapshot.clear();
            self.loop_context_snapshot.clear();
            self.idle_notify.notify_waiters();
        }
    }
}

impl Agent {
    /// Start a new loop with input messages, returning an event stream.
    ///
    /// # Stream lifecycle
    ///
    /// - On start, the conversation history (plus `input`) moves into the
    ///   spawned loop task. [`Agent::state`]'s `messages` retains a snapshot
    ///   of that same pre-run context (custom messages that cannot be
    ///   snapshotted are dropped with a warning).
    /// - As the caller drains events through [`Agent::handle_stream_event`],
    ///   completed turns are appended to `state.messages`; on `AgentEnd`,
    ///   `state.messages` is replaced with the loop's final context.
    /// - Dropping the stream **before** `AgentEnd` cancels the loop.
    ///   `state.messages` keeps the pre-run history plus any turns already
    ///   processed by the caller — nothing already observable is lost. Turns
    ///   the loop task completed but that were never drained from the stream
    ///   are discarded along with it.
    /// - Draining to `AgentEnd` is the reliable way to observe the complete
    ///   final history.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError::AlreadyRunning`] if the agent is already running.
    pub fn prompt_stream(
        &mut self,
        input: Vec<AgentMessage>,
    ) -> Result<Pin<Box<dyn Stream<Item = AgentEvent> + Send>>, AgentError> {
        self.check_not_running().inspect_err(|_| {
            warn!("prompt_stream called while agent is already running");
        })?;
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
    /// Returns [`AgentError::AlreadyRunning`] if the agent is already running,
    /// or [`AgentError::SyncInAsyncContext`] if called from within a Tokio runtime.
    pub fn prompt_sync(&mut self, input: Vec<AgentMessage>) -> Result<AgentResult, AgentError> {
        self.check_not_running()?;
        let rt = new_blocking_runtime()?;
        rt.block_on(async {
            let stream = self.start_loop(input, false)?;
            self.collect_stream(stream).await
        })
    }

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
            cache_hint: None,
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
            cache_hint: None,
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
            cache_hint: None,
        }));
        self.prompt_sync(vec![msg])
    }

    /// Continue from existing messages, returning an event stream.
    ///
    /// # Stream lifecycle
    ///
    /// The returned stream follows the same lifecycle as
    /// [`prompt_stream`](Self::prompt_stream): the history (plus any drained
    /// steering/follow-up messages) moves into the loop task while
    /// `state.messages` retains an equivalent snapshot, so dropping the
    /// stream before `AgentEnd` does not lose the conversation. Drain to
    /// `AgentEnd` to observe the complete final history.
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
    /// [`AgentError::InvalidContinue`], or [`AgentError::SyncInAsyncContext`].
    pub fn continue_sync(&mut self) -> Result<AgentResult, AgentError> {
        self.check_not_running()?;
        self.validate_continue()?;
        let rt = new_blocking_runtime()?;
        rt.block_on(async {
            let stream = self.start_loop(Vec::new(), true)?;
            self.collect_stream(stream).await
        })
    }

    pub(super) fn check_not_running(&mut self) -> Result<(), AgentError> {
        // Synchronise the observable `state.is_running` from the atomic ground
        // truth so callers that inspect `agent.state()` see an up-to-date value.
        let active = self.loop_active.load(Ordering::Acquire);
        self.state.is_running = active;
        if active {
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

    /// Build the loop config and start the agent loop, returning a wrapped stream.
    #[allow(clippy::unnecessary_wraps)]
    fn start_loop(
        &mut self,
        input: Vec<AgentMessage>,
        is_continue: bool,
    ) -> Result<Pin<Box<dyn Stream<Item = AgentEvent> + Send>>, AgentError> {
        self.state.is_running = true;
        self.state.error = None;
        self.pending_message_snapshot.clear();
        self.loop_context_snapshot.clear();
        self.loop_active.store(true, Ordering::Release);
        let generation = self.loop_generation.fetch_add(1, Ordering::AcqRel) + 1;

        let token = CancellationToken::new();
        self.abort_controller = Some(token.clone());

        let config = self.build_loop_config();
        let system_prompt = self.state.system_prompt.clone();

        let mut initial_new_messages_len = input.len();
        let messages_for_loop = if is_continue {
            let mut msgs = std::mem::take(&mut self.state.messages);
            if matches!(
                msgs.last(),
                Some(AgentMessage::Llm(LlmMessage::Assistant(_)))
            ) {
                let steering_messages = drain_messages_from_queue(&self.steering_queue);
                let follow_up_messages = drain_messages_from_queue(&self.follow_up_queue);
                initial_new_messages_len = steering_messages.len() + follow_up_messages.len();
                msgs.extend(steering_messages);
                msgs.extend(follow_up_messages);
            }
            msgs
        } else {
            let mut msgs = std::mem::take(&mut self.state.messages);
            msgs.extend(input);
            msgs
        };
        // History-loss guard: `messages_for_loop` (which now owns the entire
        // conversation history) moves into the spawned loop task below. Leave
        // an equivalent snapshot in `state.messages` so that dropping the
        // returned stream before `AgentEnd` cannot silently empty the
        // observable history. Every completion path (`collect_stream` and
        // `handle_stream_event` on `AgentEnd`) replaces `state.messages`
        // wholesale, so this snapshot is never double-counted. Custom
        // messages that cannot be snapshotted by `clone_messages_for_send`
        // are dropped here with a warning — the same limitation every other
        // state-rebuild path already has. This is the only full history pass
        // per run: `state.messages` doubles as the pause/checkpoint fallback
        // that the removed `in_flight_messages` field used to provide.
        self.state.messages = clone_messages_for_send(&messages_for_loop);

        let raw_stream = if is_continue {
            agent_loop_continue(
                messages_for_loop,
                initial_new_messages_len,
                system_prompt,
                config,
                token.clone(),
            )
        } else {
            agent_loop_with_initial_new_messages_len(
                messages_for_loop,
                initial_new_messages_len,
                system_prompt,
                config,
                token.clone(),
            )
        };

        let guarded: Pin<Box<dyn Stream<Item = AgentEvent> + Send>> = Box::pin(LoopGuardStream {
            inner: raw_stream,
            cancellation_token: token,
            loop_active: Arc::clone(&self.loop_active),
            idle_notify: Arc::clone(&self.idle_notify),
            pending_message_snapshot: Arc::clone(&self.pending_message_snapshot),
            loop_context_snapshot: Arc::clone(&self.loop_context_snapshot),
            generation,
            expected_generation: Arc::clone(&self.loop_generation),
        });
        Ok(guarded)
    }

    #[allow(clippy::type_complexity, clippy::too_many_lines)]
    fn build_loop_config(&self) -> AgentLoopConfig {
        // ── Field-mirroring drift guard ─────────────────────────────────
        // Destructure `self` exhaustively (no `..` rest pattern) so that
        // adding a field to `Agent` is a compile error right here until it
        // is either mirrored into the loop config below or consciously
        // ignored with a `_` binding. Without this, a new Agent setting
        // whose mirror line was forgotten would silently never reach the
        // running loop.
        let Self {
            id: _,
            state,
            steering_queue,
            follow_up_queue,
            listeners: _,
            abort_controller: _,
            steering_mode,
            follow_up_mode,
            stream_fn,
            convert_to_llm,
            transform_context,
            get_api_key,
            retry_strategy,
            stream_options,
            structured_output_max_retries: _,
            idle_notify: _,
            pending_message_snapshot,
            loop_context_snapshot,
            approve_tool,
            approval_mode,
            reasoning_only_nudge,
            pre_turn_policies,
            pre_dispatch_policies,
            post_turn_policies,
            post_loop_policies,
            model_stream_fns: _,
            event_forwarders: _,
            async_transform_context,
            checkpoint_store: _,
            custom_message_registry: _,
            metrics_collector,
            fallback,
            external_message_provider,
            tool_execution_policy,
            plan_mode_addendum: _,
            session_state,
            credential_resolver,
            credential_timeout,
            cache_config,
            dynamic_system_prompt,
            cost_calculator,
            loop_active: _,
            loop_generation: _,
            #[cfg(feature = "plugins")]
                plugins: _,
            agent_name,
            transfer_chain,
        } = self;

        let convert = Arc::clone(convert_to_llm);
        let convert_box: Box<dyn Fn(&AgentMessage) -> Option<LlmMessage> + Send + Sync> =
            Box::new(move |msg| convert(msg));

        let api_key_box = get_api_key.as_ref().map(|k| {
            let k = Arc::clone(k);
            let b: Box<GetApiKeyFn> = Box::new(move |provider| k(provider));
            b
        });

        let queue_provider: Arc<dyn MessageProvider> = Arc::new(QueueMessageProvider {
            steering_queue: Arc::clone(steering_queue),
            follow_up_queue: Arc::clone(follow_up_queue),
            steering_mode: *steering_mode,
            follow_up_mode: *follow_up_mode,
            pending_message_snapshot: Arc::clone(pending_message_snapshot),
        });

        let message_provider: Arc<dyn MessageProvider> =
            if let Some(external) = external_message_provider {
                Arc::new(crate::message_provider::ComposedMessageProvider::new(
                    queue_provider,
                    Arc::clone(external),
                ))
            } else {
                queue_provider
            };

        let mut config =
            AgentLoopConfig::new(state.model.clone(), Arc::clone(stream_fn), convert_box)
                .with_runtime_snapshots(
                    Arc::clone(pending_message_snapshot),
                    Arc::clone(loop_context_snapshot),
                );
        config.agent_name.clone_from(agent_name);
        config.transfer_chain.clone_from(transfer_chain);
        config.stream_options = stream_options.clone();
        config.retry_strategy = Box::new(SharedRetryStrategy(Arc::clone(retry_strategy)));
        config.tools.clone_from(&state.tools);
        config.transform_context = transform_context.as_ref().map(Arc::clone);
        config.get_api_key = api_key_box;
        config.message_provider = Some(message_provider);
        config.approve_tool = approve_tool.as_ref().map(|a| {
            let a = Arc::clone(a);
            let b: Box<ApproveToolFn> = Box::new(move |req| a(req));
            b
        });
        config.approval_mode = *approval_mode;
        config.reasoning_only_nudge = *reasoning_only_nudge;
        config.pre_turn_policies.clone_from(pre_turn_policies);
        config
            .pre_dispatch_policies
            .clone_from(pre_dispatch_policies);
        config.post_turn_policies.clone_from(post_turn_policies);
        config.post_loop_policies.clone_from(post_loop_policies);
        config.async_transform_context = async_transform_context.as_ref().map(Arc::clone);
        config.metrics_collector = metrics_collector.as_ref().map(Arc::clone);
        config.fallback.clone_from(fallback);
        config.tool_execution_policy = tool_execution_policy.clone();
        config.session_state = Arc::clone(session_state);
        config.credential_resolver = credential_resolver.as_ref().map(Arc::clone);
        config.credential_timeout = *credential_timeout;
        config.cache_config.clone_from(cache_config);
        config
            .dynamic_system_prompt
            .clone_from(dynamic_system_prompt);
        config.cost_calculator = cost_calculator.as_ref().map(Arc::clone);
        config
    }
}

fn new_blocking_runtime_with(
    build: impl FnOnce() -> std::io::Result<tokio::runtime::Runtime>,
) -> Result<tokio::runtime::Runtime, AgentError> {
    if tokio::runtime::Handle::try_current().is_ok() {
        return Err(AgentError::SyncInAsyncContext);
    }
    build().map_err(AgentError::runtime_init)
}

/// Create a new Tokio runtime for blocking sync APIs, returning
/// [`AgentError::SyncInAsyncContext`] if a runtime is already active on
/// the current thread and [`AgentError::RuntimeInit`] if runtime construction
/// fails.
pub(super) fn new_blocking_runtime() -> Result<tokio::runtime::Runtime, AgentError> {
    new_blocking_runtime_with(tokio::runtime::Runtime::new)
}

#[cfg(test)]
#[path = "invoke_tests.rs"]
mod tests;
