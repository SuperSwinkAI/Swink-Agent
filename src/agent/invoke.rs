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
use crate::loop_::{AgentEvent, AgentLoopConfig, agent_loop, agent_loop_continue};
use crate::message_provider::MessageProvider;
use crate::types::message_codec::clone_messages_for_send;
use crate::types::{AgentMessage, AgentResult, ContentBlock, LlmMessage};
use crate::util::now_timestamp;

use super::queueing::QueueMessageProvider;
use super::{Agent, SharedRetryStrategy};

// ─── LoopGuardStream ────────────────────────────────────────────────────────

/// Wrapper stream that clears the agent's `loop_active` flag when dropped.
///
/// This ensures the agent becomes idle even if the caller drops the stream
/// without draining it to `AgentEnd`. A generation counter prevents a stale
/// guard from clearing the flag for a newer run.
struct LoopGuardStream {
    inner: Pin<Box<dyn Stream<Item = AgentEvent> + Send>>,
    loop_active: Arc<AtomicBool>,
    idle_notify: Arc<Notify>,
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
            self.loop_active.store(false, Ordering::Release);
            self.idle_notify.notify_waiters();
        }
    }
}

impl Agent {
    /// Start a new loop with input messages, returning an event stream.
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
        self.loop_active.store(true, Ordering::Release);
        let generation = self.loop_generation.fetch_add(1, Ordering::AcqRel) + 1;

        let token = CancellationToken::new();
        self.abort_controller = Some(token.clone());

        let config = self.build_loop_config();
        let system_prompt = self.state.system_prompt.clone();
        let llm_source: Box<dyn Iterator<Item = &AgentMessage>> = if is_continue {
            Box::new(self.state.messages.iter())
        } else {
            Box::new(self.state.messages.iter().chain(input.iter()))
        };
        let in_flight_llm_messages: Vec<AgentMessage> = llm_source
            .filter_map(|msg| match msg {
                AgentMessage::Llm(llm) => Some(AgentMessage::Llm(llm.clone())),
                AgentMessage::Custom(_) => None,
            })
            .collect();

        let messages_for_loop = if is_continue {
            std::mem::take(&mut self.state.messages)
        } else {
            let mut msgs = std::mem::take(&mut self.state.messages);
            msgs.extend(input);
            msgs
        };
        let in_flight_messages = clone_messages_for_send(&messages_for_loop);

        let raw_stream = if is_continue {
            agent_loop_continue(messages_for_loop, system_prompt, config, token)
        } else {
            agent_loop(messages_for_loop, system_prompt, config, token)
        };

        self.in_flight_llm_messages = Some(in_flight_llm_messages);
        self.in_flight_messages = Some(in_flight_messages);

        let guarded: Pin<Box<dyn Stream<Item = AgentEvent> + Send>> = Box::pin(LoopGuardStream {
            inner: raw_stream,
            loop_active: Arc::clone(&self.loop_active),
            idle_notify: Arc::clone(&self.idle_notify),
            generation,
            expected_generation: Arc::clone(&self.loop_generation),
        });
        Ok(guarded)
    }

    #[allow(clippy::type_complexity)]
    fn build_loop_config(&self) -> AgentLoopConfig {
        let convert = Arc::clone(&self.convert_to_llm);
        let convert_box: Box<dyn Fn(&AgentMessage) -> Option<LlmMessage> + Send + Sync> =
            Box::new(move |msg| convert(msg));

        let transform = self.transform_context.as_ref().map(Arc::clone);

        let api_key_box = self.get_api_key.as_ref().map(|k| {
            let k = Arc::clone(k);
            let b: Box<GetApiKeyFn> = Box::new(move |provider| k(provider));
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
            agent_name: self.agent_name.clone(),
            transfer_chain: self.transfer_chain.clone(),
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
            pre_turn_policies: self.pre_turn_policies.clone(),
            pre_dispatch_policies: self.pre_dispatch_policies.clone(),
            post_turn_policies: self.post_turn_policies.clone(),
            post_loop_policies: self.post_loop_policies.clone(),
            async_transform_context: self.async_transform_context.as_ref().map(Arc::clone),
            metrics_collector: self.metrics_collector.as_ref().map(Arc::clone),
            fallback: self.fallback.clone(),
            tool_execution_policy: self.tool_execution_policy.clone(),
            session_state: Arc::clone(&self.session_state),
            credential_resolver: self.credential_resolver.as_ref().map(Arc::clone),
            cache_config: self.cache_config.clone(),
            cache_state: std::sync::Mutex::new(crate::context_cache::CacheState::new()),
            dynamic_system_prompt: self.dynamic_system_prompt.clone(),
        }
    }
}

/// Create a new Tokio runtime for blocking sync APIs, returning
/// [`AgentError::SyncInAsyncContext`] if a runtime is already active on
/// the current thread.
pub(super) fn new_blocking_runtime() -> Result<tokio::runtime::Runtime, AgentError> {
    if tokio::runtime::Handle::try_current().is_ok() {
        return Err(AgentError::SyncInAsyncContext);
    }
    Ok(tokio::runtime::Runtime::new().expect("failed to create tokio runtime"))
}
