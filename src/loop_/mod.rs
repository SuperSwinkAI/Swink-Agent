//! Core agent loop execution engine.
//!
//! Implements the nested inner/outer loop, tool dispatch, steering/follow-up
//! injection, event emission, retry integration, error/abort handling, and max
//! tokens recovery. Stateless — all state is passed in via [`AgentLoopConfig`].

mod stream;
mod tool_dispatch;
mod turn;

use std::error::Error as _;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, info, info_span};

use crate::async_context_transformer::AsyncContextTransformer;
use crate::error::AgentError;
use crate::fallback::ModelFallback;
use crate::message_provider::MessageProvider;
use crate::retry::RetryStrategy;
use crate::stream::{AssistantMessageDelta, StreamFn, StreamOptions};
use crate::tool::AgentTool;
use crate::tool::{AgentToolResult, ApprovalMode, ToolApproval, ToolApprovalRequest};
use crate::tool_execution_policy::ToolExecutionPolicy;
use crate::types::{
    AgentMessage, AssistantMessage, LlmMessage, ModelSpec, StopReason, ToolResultMessage,
};
use crate::util::now_timestamp;

// ─── Constants ───────────────────────────────────────────────────────────────

/// Sentinel value used to signal context overflow between `handle_stream_result`
/// and `run_single_turn`.
pub const CONTEXT_OVERFLOW_SENTINEL: &str = "__context_overflow__";

/// Channel capacity for agent events. Sized to handle burst streaming
/// without backpressure under normal operation.
const EVENT_CHANNEL_CAPACITY: usize = 256;

// ─── Type Aliases ────────────────────────────────────────────────────────────

/// Converts an `AgentMessage` to an optional `LlmMessage` for the provider.
type ConvertToLlmFn = dyn Fn(&AgentMessage) -> Option<LlmMessage> + Send + Sync;

/// Async API key resolution callback.
type GetApiKeyFn =
    dyn Fn(&str) -> Pin<Box<dyn Future<Output = Option<String>> + Send>> + Send + Sync;

/// Async callback for approving or rejecting individual tool calls.
pub type ApproveToolFn =
    dyn Fn(ToolApprovalRequest) -> Pin<Box<dyn Future<Output = ToolApproval> + Send>> + Send + Sync;

// ─── TurnEndReason ───────────────────────────────────────────────────────────

/// Why a turn ended.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnEndReason {
    /// Assistant completed without requesting tool calls.
    Complete,
    /// Tools were executed (loop continues).
    ToolsExecuted,
    /// Turn was interrupted by a steering message during tool execution.
    SteeringInterrupt,
    /// LLM returned an error stop reason.
    Error,
    /// External cancellation via `CancellationToken`.
    Cancelled,
    /// Stream was aborted mid-generation.
    Aborted,
}

// ─── AgentEvent ──────────────────────────────────────────────────────────────

/// Fine-grained lifecycle event emitted by the agent loop.
///
/// Consumers subscribe to these events for observability, UI updates, and
/// logging. The harness never calls back into application logic for display
/// concerns.
#[non_exhaustive]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum AgentEvent {
    /// Emitted once when the loop begins.
    AgentStart,

    /// Emitted once when the loop exits, carrying the final message context.
    AgentEnd { messages: Arc<Vec<AgentMessage>> },

    /// Emitted at the beginning of each assistant turn.
    TurnStart,

    /// Emitted at the end of each turn with the assistant message and tool results.
    TurnEnd {
        assistant_message: AssistantMessage,
        tool_results: Vec<ToolResultMessage>,
        reason: TurnEndReason,
        /// Full context snapshot at the turn boundary for replay/auditing.
        snapshot: crate::types::TurnSnapshot,
    },

    /// Emitted after context transform, before the LLM streaming call.
    /// Allows plugins to observe/log the final prompt.
    BeforeLlmCall {
        system_prompt: String,
        messages: Vec<LlmMessage>,
        model: ModelSpec,
    },

    /// Emitted when a message begins streaming.
    MessageStart,

    /// Emitted for each incremental delta during assistant streaming.
    MessageUpdate { delta: AssistantMessageDelta },

    /// Emitted when a message is complete.
    MessageEnd { message: AssistantMessage },

    /// Emitted when a tool call begins execution.
    ToolExecutionStart {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },

    /// Emitted for intermediate partial results from a streaming tool.
    ToolExecutionUpdate { partial: AgentToolResult },

    /// Emitted when a tool call is pending approval.
    ToolApprovalRequested {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },

    /// Emitted when a tool call approval decision is made.
    ToolApprovalResolved {
        id: String,
        name: String,
        approved: bool,
    },

    /// Emitted when a tool call completes.
    ToolExecutionEnd {
        result: AgentToolResult,
        is_error: bool,
    },

    /// Emitted when context compaction drops messages.
    ContextCompacted {
        report: crate::context_transformer::CompactionReport,
    },

    /// Emitted when the agent falls back to a different model after exhausting
    /// retries on the current one.
    ModelFallback {
        from_model: ModelSpec,
        to_model: ModelSpec,
    },

    /// Emitted when the agent switches to a different model during a retry cycle.
    ModelCycled {
        old: ModelSpec,
        new: ModelSpec,
        reason: String,
    },

    /// A custom event emitted via [`Agent::emit`](crate::Agent::emit).
    Custom(crate::emit::Emission),
}

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

// ─── Entry Points ────────────────────────────────────────────────────────────

/// Start a new agent loop with prompt messages.
///
/// Creates an initial context with the prompt messages, then runs the loop.
/// Returns a stream of `AgentEvent` values.
#[must_use]
pub fn agent_loop(
    prompt_messages: Vec<AgentMessage>,
    system_prompt: String,
    config: AgentLoopConfig,
    cancellation_token: CancellationToken,
) -> Pin<Box<dyn Stream<Item = AgentEvent> + Send>> {
    run_loop(prompt_messages, system_prompt, config, cancellation_token)
}

/// Resume an agent loop from existing messages.
///
/// Resumes from existing messages (no new prompt), calls the loop.
/// Returns a stream of `AgentEvent` values.
#[must_use]
pub fn agent_loop_continue(
    messages: Vec<AgentMessage>,
    system_prompt: String,
    config: AgentLoopConfig,
    cancellation_token: CancellationToken,
) -> Pin<Box<dyn Stream<Item = AgentEvent> + Send>> {
    run_loop(messages, system_prompt, config, cancellation_token)
}

// ─── Internal Loop ───────────────────────────────────────────────────────────

/// The core loop implementation. Spawns a task that drives the loop and sends
/// events through an mpsc channel, returning a stream of events.
fn run_loop(
    initial_messages: Vec<AgentMessage>,
    system_prompt: String,
    config: AgentLoopConfig,
    cancellation_token: CancellationToken,
) -> Pin<Box<dyn Stream<Item = AgentEvent> + Send>> {
    let (tx, rx) = mpsc::channel::<AgentEvent>(EVENT_CHANNEL_CAPACITY);

    tokio::spawn(async move {
        run_loop_inner(
            initial_messages,
            system_prompt,
            config,
            cancellation_token,
            tx,
        )
        .await;
    });

    Box::pin(ReceiverStream::new(rx))
}

/// Send an event through the channel. Returns false if the receiver is dropped.
pub async fn emit(tx: &mpsc::Sender<AgentEvent>, event: AgentEvent) -> bool {
    tx.send(event).await.is_ok()
}

// ─── Loop State ──────────────────────────────────────────────────────────────

/// Mutable state threaded through the loop iterations.
pub struct LoopState {
    pub context_messages: Vec<AgentMessage>,
    pub pending_messages: Vec<AgentMessage>,
    pub overflow_signal: bool,
    pub turn_index: usize,
    pub accumulated_usage: crate::types::Usage,
    pub accumulated_cost: crate::types::Cost,
    /// The last assistant message from a completed turn (for policy checks).
    pub last_assistant_message: Option<AssistantMessage>,
    /// Tool results from the last completed turn (for post-turn hook).
    pub last_tool_results: Vec<ToolResultMessage>,
}

// ─── run_loop_inner ──────────────────────────────────────────────────────────

/// The actual loop logic running inside the spawned task.
#[allow(clippy::too_many_lines)]
async fn run_loop_inner(
    initial_messages: Vec<AgentMessage>,
    system_prompt: String,
    config: AgentLoopConfig,
    cancellation_token: CancellationToken,
    tx: mpsc::Sender<AgentEvent>,
) {
    let config = Arc::new(config);
    let span = info_span!(
        "agent_loop",
        model_id = %config.model.model_id,
        provider = %config.model.provider,
        tool_count = config.tools.len(),
        message_count = initial_messages.len(),
    );
    async {
        info!(
            model = %config.model.model_id,
            provider = %config.model.provider,
            tools = config.tools.len(),
            "starting agent loop"
        );
        let mut state = LoopState {
            context_messages: initial_messages,
            pending_messages: Vec::new(),
            overflow_signal: false,
            turn_index: 0,
            accumulated_usage: crate::types::Usage::default(),
            accumulated_cost: crate::types::Cost::default(),
            last_assistant_message: None,
            last_tool_results: Vec::new(),
        };

        // 1. Emit AgentStart
        if !emit(&tx, AgentEvent::AgentStart).await {
            return;
        }

        // 2. Outer loop (follow-up phase)
        'outer: loop {
            // Inner loop (turn + tool phase)
            'inner: loop {
                let turn_result = turn::run_single_turn(
                    &config,
                    &mut state,
                    &system_prompt,
                    &cancellation_token,
                    &tx,
                )
                .await;

                let should_break = match turn_result {
                    TurnOutcome::ContinueInner => {
                        state.turn_index += 1;
                        false
                    }
                    TurnOutcome::BreakInner => true,
                    TurnOutcome::Return => return,
                };

                // Post-turn policies: invoke after each completed turn
                if let Some(ref msg) = state.last_assistant_message {
                    use crate::policy::{
                        PolicyContext, PolicyVerdict, TurnPolicyContext, run_post_turn_policies,
                    };

                    let policy_ctx = PolicyContext {
                        turn_index: state.turn_index,
                        accumulated_usage: &state.accumulated_usage,
                        accumulated_cost: &state.accumulated_cost,
                        message_count: state.context_messages.len(),
                        overflow_signal: state.overflow_signal,
                        new_messages: &[], // current-turn data is in TurnPolicyContext
                    };
                    let turn_ctx = TurnPolicyContext {
                        assistant_message: msg,
                        tool_results: &state.last_tool_results,
                        stop_reason: msg.stop_reason,
                    };
                    match run_post_turn_policies(
                        &config.post_turn_policies,
                        &policy_ctx,
                        &turn_ctx,
                    ) {
                        PolicyVerdict::Continue => {}
                        PolicyVerdict::Stop(reason) => {
                            info!("post-turn policy stopped agent: {reason}");
                            break 'inner;
                        }
                        PolicyVerdict::Inject(msgs) => {
                            state.pending_messages.extend(msgs);
                        }
                    }
                }

                if should_break {
                    break 'inner;
                }
            }

            // Post-loop policies: evaluate after inner loop exits
            {
                use crate::policy::{PolicyContext, PolicyVerdict, run_post_loop_policies};

                let policy_ctx = PolicyContext {
                    turn_index: state.turn_index,
                    accumulated_usage: &state.accumulated_usage,
                    accumulated_cost: &state.accumulated_cost,
                    message_count: state.context_messages.len(),
                    overflow_signal: state.overflow_signal,
                    new_messages: &[], // no new messages at post-loop
                };
                match run_post_loop_policies(&config.post_loop_policies, &policy_ctx) {
                    PolicyVerdict::Continue => {}
                    PolicyVerdict::Stop(_reason) => {
                        let _ = emit(
                            &tx,
                            AgentEvent::AgentEnd {
                                messages: Arc::new(state.context_messages),
                            },
                        )
                        .await;
                        info!("post-loop policy stopped agent");
                        return;
                    }
                    PolicyVerdict::Inject(msgs) => {
                        state.pending_messages.extend(msgs);
                        continue 'outer;
                    }
                }
            }

            // Outer loop: poll follow-up messages
            if let Some(ref provider) = config.message_provider {
                let msgs = provider.poll_follow_up();
                if !msgs.is_empty() {
                    state.pending_messages.extend(msgs);
                    continue 'outer;
                }
            }

            // No follow-up → emit AgentEnd and exit
            let _ = emit(
                &tx,
                AgentEvent::AgentEnd {
                    messages: Arc::new(state.context_messages),
                },
            )
            .await;
            info!("agent loop finished");
            return;
        }
    }
    .instrument(span)
    .await;
}

/// Outcome of a single turn execution within the inner loop.
pub enum TurnOutcome {
    /// Continue to the next inner-loop iteration (tool results need processing).
    ContinueInner,
    /// Break out of the inner loop (no tool calls, check follow-ups).
    BreakInner,
    /// Return from the entire loop (channel closed, error, or abort).
    Return,
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Info about a tool call extracted from the assistant message.
pub struct ToolCallInfo {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    pub is_incomplete: bool,
}

/// Result of streaming an assistant response.
#[allow(clippy::large_enum_variant)]
pub enum StreamResult {
    Message(AssistantMessage),
    ContextOverflow,
    Aborted,
    ChannelClosed,
}

/// Outcome of concurrent tool execution.
pub enum ToolExecOutcome {
    Completed {
        results: Vec<ToolResultMessage>,
        tool_metrics: Vec<crate::metrics::ToolExecMetrics>,
    },
    SteeringInterrupt {
        completed: Vec<ToolResultMessage>,
        cancelled: Vec<ToolResultMessage>,
        steering_messages: Vec<AgentMessage>,
        tool_metrics: Vec<crate::metrics::ToolExecMetrics>,
    },
    ChannelClosed,
}

/// Build a terminal `AssistantMessage` with the given stop reason and message.
fn build_terminal_message(
    model: &ModelSpec,
    stop_reason: StopReason,
    error_message: String,
) -> AssistantMessage {
    AssistantMessage {
        content: vec![],
        provider: model.provider.clone(),
        model_id: model.model_id.clone(),
        usage: crate::types::Usage::default(),
        cost: crate::types::Cost::default(),
        stop_reason,
        error_message: Some(error_message),
        timestamp: now_timestamp(),
    }
}

/// Build an aborted `AssistantMessage`.
pub fn build_abort_message(model: &ModelSpec) -> AssistantMessage {
    build_terminal_message(
        model,
        StopReason::Aborted,
        "operation aborted via cancellation token".to_string(),
    )
}

/// Build an error `AssistantMessage` from a `AgentError`.
pub fn build_error_message(model: &ModelSpec, error: &AgentError) -> AssistantMessage {
    build_terminal_message(model, StopReason::Error, format_error_with_sources(error))
}

pub fn format_error_with_sources(error: &AgentError) -> String {
    let mut message = error.to_string();
    let mut source = error.source();

    while let Some(err) = source {
        let source_message = err.to_string();
        if !source_message.is_empty() && !message.contains(&source_message) {
            message.push_str(": ");
            message.push_str(&source_message);
        }
        source = err.source();
    }

    message
}

/// Classify an `AssistantMessageEvent::Error` into a `AgentError`.
pub fn classify_stream_error(error_message: &str, stop_reason: StopReason) -> AgentError {
    let lower = error_message.to_lowercase();
    if lower.contains("context window") || lower.contains("context_length_exceeded") {
        return AgentError::ContextWindowOverflow {
            model: String::new(),
        };
    }
    if lower.contains("rate limit") || lower.contains("429") || lower.contains("throttl") {
        return AgentError::ModelThrottled;
    }
    if stop_reason == StopReason::Aborted {
        return AgentError::Aborted;
    }
    AgentError::StreamError {
        source: Box::new(std::io::Error::other(error_message.to_string())),
    }
}
