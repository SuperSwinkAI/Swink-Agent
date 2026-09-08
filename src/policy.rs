//! Configurable policy slots for the agent loop.
//!
//! Provides four policy slots at natural seam points in the agent loop:
//! - **`PreTurn`** (Slot 1): Before each LLM call — guards and pre-conditions.
//! - **`PreDispatch`** (Slot 2): Per tool call, before approval — validation and argument mutation.
//! - **`PostTurn`** (Slot 3): After each completed turn — persistence, steering, stop conditions.
//! - **`PostLoop`** (Slot 4): After the inner loop exits — cleanup before follow-up polling.
//!
//! Each slot accepts a `Vec<Arc<dyn Trait>>` of policy implementations, evaluated in order.
//! The default is empty vecs — no policies, anything goes.
//!
//! Two verdict enums enforce Skip-only-in-PreDispatch at compile time:
//! - [`PolicyVerdict`]: Used by `PreTurn`, `PostTurn`, and `PostLoop` (no Skip variant).
//! - [`PreDispatchVerdict`]: Used by `PreDispatch` (includes Skip).
//!
//! The slot runner catches panics via `catch_unwind` (using `AssertUnwindSafe`),
//! so policy traits only require `Send + Sync` — implementors do not need `UnwindSafe`.
#![forbid(unsafe_code)]

use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::Arc;

use tracing::{debug, warn};

use crate::types::{
    AgentMessage, AssistantMessage, Cost, ModelSpec, StopReason, ToolResultMessage, Usage,
};

// ─── Verdict Enums ──────────────────────────────────────────────────────────

/// Outcome of a policy evaluation for `PreTurn`, `PostTurn`, and `PostLoop` slots.
///
/// Does not include `Skip` — that is only available in [`PreDispatchVerdict`].
#[non_exhaustive]
#[derive(Debug)]
pub enum PolicyVerdict {
    /// Proceed normally.
    Continue,
    /// Stop the loop gracefully with a reason.
    Stop(String),
    /// Add messages to the pending queue and continue.
    Inject(Vec<AgentMessage>),
}

/// Outcome of a `PreDispatch` policy evaluation.
///
/// Includes `Skip` for per-tool-call rejection, in addition to the
/// verdicts available in [`PolicyVerdict`].
#[non_exhaustive]
#[derive(Debug)]
pub enum PreDispatchVerdict {
    /// Proceed normally.
    Continue,
    /// Abort the entire tool batch and stop the loop.
    Stop(String),
    /// Add messages to the pending queue and continue.
    Inject(Vec<AgentMessage>),
    /// Skip this tool call, returning the error text to the LLM.
    Skip(String),
}

// ─── Context Structs ────────────────────────────────────────────────────────

/// Shared read-only context available to every policy evaluation.
#[non_exhaustive]
#[derive(Debug)]
pub struct PolicyContext<'a> {
    /// Zero-based index of the current/completed turn.
    pub turn_index: usize,
    /// Accumulated token usage across all turns.
    pub accumulated_usage: &'a Usage,
    /// Accumulated cost across all turns.
    pub accumulated_cost: &'a Cost,
    /// Number of messages in context.
    pub message_count: usize,
    /// Whether context overflow was signaled.
    pub overflow_signal: bool,
    /// Messages added since the last policy evaluation for this slot.
    ///
    /// - **`PreTurn`**: user/pending messages appended since the previous turn.
    /// - **`PostTurn`** / **`PostLoop`**: empty — current-turn data is in [`TurnPolicyContext`].
    ///
    /// Policies should only scan this slice, never the full session history,
    /// to avoid redundant work on messages that have already been evaluated.
    pub new_messages: &'a [AgentMessage],
    /// Read-only access to the session state.
    pub state: &'a crate::SessionState,
}

impl<'a> PolicyContext<'a> {
    /// Create a new policy context.
    #[must_use]
    pub const fn new(
        turn_index: usize,
        accumulated_usage: &'a Usage,
        accumulated_cost: &'a Cost,
        message_count: usize,
        overflow_signal: bool,
        new_messages: &'a [AgentMessage],
        state: &'a crate::SessionState,
    ) -> Self {
        Self {
            turn_index,
            accumulated_usage,
            accumulated_cost,
            message_count,
            overflow_signal,
            new_messages,
            state,
        }
    }
}

/// Combined context for `PreDispatch` policies.
///
/// Contains only the data reliably available during tool dispatch — the per-call
/// fields and read-only session state. Loop-level metrics (turn index, accumulated
/// usage/cost, message count, overflow signal) are intentionally excluded: they are
/// not tracked at the tool dispatch call site, and fabricating placeholder values
/// would give policies incorrect data to reason from.
#[non_exhaustive]
pub struct ToolDispatchContext<'a> {
    /// Name of the tool being called.
    pub tool_name: &'a str,
    /// Unique identifier for this tool call.
    pub tool_call_id: &'a str,
    /// Mutable reference to tool call arguments (policies may rewrite them).
    pub arguments: &'a mut serde_json::Value,
    /// Working directory the tool will resolve relative paths against, when known.
    pub execution_root: Option<&'a Path>,
    /// Read-only access to the session state.
    pub state: &'a crate::SessionState,
}

impl<'a> ToolDispatchContext<'a> {
    /// Create a new tool dispatch context.
    #[must_use]
    pub const fn new(
        tool_name: &'a str,
        tool_call_id: &'a str,
        arguments: &'a mut serde_json::Value,
        execution_root: Option<&'a Path>,
        state: &'a crate::SessionState,
    ) -> Self {
        Self {
            tool_name,
            tool_call_id,
            arguments,
            execution_root,
            state,
        }
    }
}

impl std::fmt::Debug for ToolDispatchContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolDispatchContext")
            .field("tool_name", &self.tool_name)
            .field("tool_call_id", &self.tool_call_id)
            .field("execution_root", &self.execution_root)
            .field("arguments", &"<redacted>")
            .finish()
    }
}

/// Per-turn context for `PostTurn` policies.
#[non_exhaustive]
#[derive(Debug)]
pub struct TurnPolicyContext<'a> {
    /// The assistant message from the completed turn.
    pub assistant_message: &'a AssistantMessage,
    /// Tool results produced during this turn.
    pub tool_results: &'a [ToolResultMessage],
    /// Why the turn ended.
    pub stop_reason: StopReason,
    /// The system prompt active during this turn.
    pub system_prompt: &'a str,
    /// The model specification active during this turn.
    pub model_spec: &'a ModelSpec,
    /// The committed conversation history for the completed turn.
    ///
    /// This always includes the current turn's assistant message and any tool
    /// results before `PostTurn` policies run, regardless of whether the turn
    /// ended with plain text, tool execution, or transfer termination.
    pub context_messages: &'a [AgentMessage],
}

impl<'a> TurnPolicyContext<'a> {
    /// Create a new per-turn policy context.
    #[must_use]
    pub const fn new(
        assistant_message: &'a AssistantMessage,
        tool_results: &'a [ToolResultMessage],
        stop_reason: StopReason,
        system_prompt: &'a str,
        model_spec: &'a ModelSpec,
        context_messages: &'a [AgentMessage],
    ) -> Self {
        Self {
            assistant_message,
            tool_results,
            stop_reason,
            system_prompt,
            model_spec,
            context_messages,
        }
    }
}

// ─── Slot Traits ────────────────────────────────────────────────────────────

/// Slot 1: Evaluated before each LLM call.
///
/// Use for guards and pre-conditions (budget limits, turn caps, rate limiting).
/// Trait bounds are `Send + Sync` only — the slot runner handles `catch_unwind`
/// via `AssertUnwindSafe`, so implementors do not need `UnwindSafe`.
///
/// Stateful policies should use interior mutability (`Mutex`, atomics).
pub trait PreTurnPolicy: Send + Sync {
    /// Policy identifier for tracing and debugging.
    fn name(&self) -> &str;
    /// Evaluate the policy. Returns [`PolicyVerdict`].
    fn evaluate(&self, ctx: &PolicyContext<'_>) -> PolicyVerdict;
}

/// Slot 2: Evaluated per tool call, before approval and execution.
///
/// Can inspect and mutate tool call arguments via [`ToolDispatchContext`].
/// Returns [`PreDispatchVerdict`] which includes `Skip` for per-tool rejection.
pub trait PreDispatchPolicy: Send + Sync {
    /// Policy identifier for tracing and debugging.
    fn name(&self) -> &str;
    /// Evaluate the policy. Returns [`PreDispatchVerdict`].
    fn evaluate(&self, ctx: &mut ToolDispatchContext<'_>) -> PreDispatchVerdict;
}

/// Slot 3: Evaluated after each completed turn.
///
/// Use for persistence, loop detection, dynamic stop conditions, or steering injection.
pub trait PostTurnPolicy: Send + Sync {
    /// Policy identifier for tracing and debugging.
    fn name(&self) -> &str;
    /// Evaluate the policy. Returns [`PolicyVerdict`].
    fn evaluate(&self, ctx: &PolicyContext<'_>, turn: &TurnPolicyContext<'_>) -> PolicyVerdict;
}

/// Slot 4: Evaluated after the inner loop exits, before follow-up polling.
///
/// Use for cleanup, cooldown, or rate limiting between outer loop iterations.
pub trait PostLoopPolicy: Send + Sync {
    /// Policy identifier for tracing and debugging.
    fn name(&self) -> &str;
    /// Evaluate the policy. Returns [`PolicyVerdict`].
    fn evaluate(&self, ctx: &PolicyContext<'_>) -> PolicyVerdict;
}

// ─── Slot Runners ───────────────────────────────────────────────────────────

/// Evaluate `PreTurn`, `PostTurn`, or `PostLoop` policies in order.
///
/// - **Stop** short-circuits: first Stop wins, remaining policies don't run.
/// - **Inject** accumulates: all non-short-circuited policies contribute messages.
/// - **Panics** are caught via `catch_unwind` and treated as Continue.
pub fn run_policies(policies: &[Arc<dyn PreTurnPolicy>], ctx: &PolicyContext<'_>) -> PolicyVerdict {
    run_policies_inner(policies.iter().map(std::convert::AsRef::as_ref), ctx)
}

/// Evaluate `PostTurn` policies in order.
pub fn run_post_turn_policies(
    policies: &[Arc<dyn PostTurnPolicy>],
    ctx: &PolicyContext<'_>,
    turn: &TurnPolicyContext<'_>,
) -> PolicyVerdict {
    let mut injections: Vec<AgentMessage> = Vec::new();

    for policy in policies {
        let policy_name = policy.name().to_string();
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| policy.evaluate(ctx, turn)));

        match result {
            Ok(PolicyVerdict::Continue) => {}
            Ok(PolicyVerdict::Stop(reason)) => {
                debug!(policy = %policy_name, reason = %reason, "policy stopped loop");
                return PolicyVerdict::Stop(reason);
            }
            Ok(PolicyVerdict::Inject(msgs)) => {
                injections.extend(msgs);
            }
            Err(_) => {
                warn!(policy = %policy_name, "policy panicked during evaluation, skipping");
            }
        }
    }

    if injections.is_empty() {
        PolicyVerdict::Continue
    } else {
        PolicyVerdict::Inject(injections)
    }
}

/// Evaluate `PostLoop` policies in order.
pub fn run_post_loop_policies(
    policies: &[Arc<dyn PostLoopPolicy>],
    ctx: &PolicyContext<'_>,
) -> PolicyVerdict {
    let mut injections: Vec<AgentMessage> = Vec::new();

    for policy in policies {
        let policy_name = policy.name().to_string();
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| policy.evaluate(ctx)));

        match result {
            Ok(PolicyVerdict::Continue) => {}
            Ok(PolicyVerdict::Stop(reason)) => {
                debug!(policy = %policy_name, reason = %reason, "policy stopped loop");
                return PolicyVerdict::Stop(reason);
            }
            Ok(PolicyVerdict::Inject(msgs)) => {
                injections.extend(msgs);
            }
            Err(_) => {
                warn!(policy = %policy_name, "policy panicked during evaluation, skipping");
            }
        }
    }

    if injections.is_empty() {
        PolicyVerdict::Continue
    } else {
        PolicyVerdict::Inject(injections)
    }
}

/// Internal runner for `PreTurn` policies (same signature pattern).
fn run_policies_inner<'a>(
    policies: impl Iterator<Item = &'a dyn PreTurnPolicy>,
    ctx: &PolicyContext<'_>,
) -> PolicyVerdict {
    let mut injections: Vec<AgentMessage> = Vec::new();

    for policy in policies {
        let policy_name = policy.name().to_string();
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| policy.evaluate(ctx)));

        match result {
            Ok(PolicyVerdict::Continue) => {}
            Ok(PolicyVerdict::Stop(reason)) => {
                debug!(policy = %policy_name, reason = %reason, "policy stopped loop");
                return PolicyVerdict::Stop(reason);
            }
            Ok(PolicyVerdict::Inject(msgs)) => {
                injections.extend(msgs);
            }
            Err(_) => {
                warn!(policy = %policy_name, "policy panicked during evaluation, skipping");
            }
        }
    }

    if injections.is_empty() {
        PolicyVerdict::Continue
    } else {
        PolicyVerdict::Inject(injections)
    }
}

/// Evaluate `PreDispatch` policies for a single tool call.
///
/// - **Stop** short-circuits: aborts the entire tool batch.
/// - **Skip** short-circuits: skips this tool call with error text.
/// - **Inject** accumulates.
/// - **Panics** are caught, argument mutations are rolled back, and evaluation continues.
pub fn run_pre_dispatch_policies(
    policies: &[Arc<dyn PreDispatchPolicy>],
    ctx: &mut ToolDispatchContext<'_>,
) -> PreDispatchVerdict {
    let mut injections: Vec<AgentMessage> = Vec::new();

    for policy in policies {
        let policy_name = policy.name().to_string();
        let argument_snapshot = ctx.arguments.clone();
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| policy.evaluate(ctx)));

        match result {
            Ok(PreDispatchVerdict::Continue) => {}
            Ok(PreDispatchVerdict::Stop(reason)) => {
                debug!(policy = %policy_name, reason = %reason, "policy stopped loop (pre-dispatch)");
                return PreDispatchVerdict::Stop(reason);
            }
            Ok(PreDispatchVerdict::Skip(error_text)) => {
                debug!(policy = %policy_name, "policy skipped tool call");
                return PreDispatchVerdict::Skip(error_text);
            }
            Ok(PreDispatchVerdict::Inject(msgs)) => {
                injections.extend(msgs);
            }
            Err(_) => {
                *ctx.arguments = argument_snapshot;
                warn!(policy = %policy_name, "policy panicked during evaluation, skipping");
            }
        }
    }

    if injections.is_empty() {
        PreDispatchVerdict::Continue
    } else {
        PreDispatchVerdict::Inject(injections)
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
