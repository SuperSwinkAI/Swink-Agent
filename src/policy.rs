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
use std::sync::Arc;

use tracing::{debug, warn};

use crate::types::{
    AgentMessage, AssistantMessage, Cost, ModelSpec, StopReason, ToolResultMessage, Usage,
};

// ─── Verdict Enums ──────────────────────────────────────────────────────────

/// Outcome of a policy evaluation for `PreTurn`, `PostTurn`, and `PostLoop` slots.
///
/// Does not include `Skip` — that is only available in [`PreDispatchVerdict`].
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

/// Combined context for `PreDispatch` policies.
///
/// Contains only the data reliably available during tool dispatch — the per-call
/// fields and read-only session state. Loop-level metrics (turn index, accumulated
/// usage/cost, message count, overflow signal) are intentionally excluded: they are
/// not tracked at the tool dispatch call site, and fabricating placeholder values
/// would give policies incorrect data to reason from.
pub struct ToolDispatchContext<'a> {
    /// Name of the tool being called.
    pub tool_name: &'a str,
    /// Unique identifier for this tool call.
    pub tool_call_id: &'a str,
    /// Mutable reference to tool call arguments (policies may rewrite them).
    pub arguments: &'a mut serde_json::Value,
    /// Read-only access to the session state.
    pub state: &'a crate::SessionState,
}

impl std::fmt::Debug for ToolDispatchContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolDispatchContext")
            .field("tool_name", &self.tool_name)
            .field("tool_call_id", &self.tool_call_id)
            .field("arguments", &"<redacted>")
            .finish()
    }
}

/// Per-turn context for `PostTurn` policies.
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
    /// All context messages (the full conversation history).
    pub context_messages: &'a [AgentMessage],
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
/// - **Panics** are caught and treated as Continue.
pub fn run_pre_dispatch_policies(
    policies: &[Arc<dyn PreDispatchPolicy>],
    ctx: &mut ToolDispatchContext<'_>,
) -> PreDispatchVerdict {
    let mut injections: Vec<AgentMessage> = Vec::new();

    for policy in policies {
        let policy_name = policy.name().to_string();
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
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ── Test helpers ──

    struct TestPolicy {
        policy_name: String,
        make_verdict: Box<dyn Fn() -> PolicyVerdict + Send + Sync>,
        call_count: AtomicUsize,
    }

    impl TestPolicy {
        fn new(name: &str, make: impl Fn() -> PolicyVerdict + Send + Sync + 'static) -> Self {
            Self {
                policy_name: name.to_string(),
                make_verdict: Box::new(make),
                call_count: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    impl PreTurnPolicy for TestPolicy {
        fn name(&self) -> &str {
            &self.policy_name
        }
        fn evaluate(&self, _ctx: &PolicyContext<'_>) -> PolicyVerdict {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            (self.make_verdict)()
        }
    }

    struct PanickingPolicy;
    impl PreTurnPolicy for PanickingPolicy {
        fn name(&self) -> &'static str {
            "panicker"
        }
        fn evaluate(&self, _ctx: &PolicyContext<'_>) -> PolicyVerdict {
            panic!("policy intentionally panicked");
        }
    }

    struct TestPreDispatchPolicy {
        policy_name: String,
        make_verdict: Box<dyn Fn() -> PreDispatchVerdict + Send + Sync>,
        call_count: AtomicUsize,
    }

    impl TestPreDispatchPolicy {
        fn new(name: &str, make: impl Fn() -> PreDispatchVerdict + Send + Sync + 'static) -> Self {
            Self {
                policy_name: name.to_string(),
                make_verdict: Box::new(make),
                call_count: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    impl PreDispatchPolicy for TestPreDispatchPolicy {
        fn name(&self) -> &str {
            &self.policy_name
        }
        fn evaluate(&self, _ctx: &mut ToolDispatchContext<'_>) -> PreDispatchVerdict {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            (self.make_verdict)()
        }
    }

    struct PanickingPreDispatchPolicy;
    impl PreDispatchPolicy for PanickingPreDispatchPolicy {
        fn name(&self) -> &'static str {
            "panicker"
        }
        fn evaluate(&self, _ctx: &mut ToolDispatchContext<'_>) -> PreDispatchVerdict {
            panic!("pre-dispatch policy panicked");
        }
    }

    struct MutatingPreDispatchPolicy;
    impl PreDispatchPolicy for MutatingPreDispatchPolicy {
        fn name(&self) -> &'static str {
            "mutator"
        }
        fn evaluate(&self, ctx: &mut ToolDispatchContext<'_>) -> PreDispatchVerdict {
            if let Some(obj) = ctx.arguments.as_object_mut() {
                obj.insert("injected".to_string(), serde_json::json!("by_policy"));
            }
            PreDispatchVerdict::Continue
        }
    }

    struct VerifyingPreDispatchPolicy {
        expected_key: String,
    }
    impl PreDispatchPolicy for VerifyingPreDispatchPolicy {
        fn name(&self) -> &'static str {
            "verifier"
        }
        fn evaluate(&self, ctx: &mut ToolDispatchContext<'_>) -> PreDispatchVerdict {
            if ctx.arguments.get(&self.expected_key).is_some() {
                PreDispatchVerdict::Continue
            } else {
                PreDispatchVerdict::Skip(format!("missing key: {}", self.expected_key))
            }
        }
    }

    fn test_message() -> AgentMessage {
        AgentMessage::Llm(crate::types::LlmMessage::User(crate::types::UserMessage {
            content: vec![],
            timestamp: 0,
            cache_hint: None,
        }))
    }

    fn test_context() -> (Usage, Cost) {
        (Usage::default(), Cost::default())
    }

    fn make_ctx<'a>(
        usage: &'a Usage,
        cost: &'a Cost,
        state: &'a crate::SessionState,
    ) -> PolicyContext<'a> {
        PolicyContext {
            turn_index: 0,
            accumulated_usage: usage,
            accumulated_cost: cost,
            message_count: 5,
            overflow_signal: false,
            new_messages: &[],
            state,
        }
    }

    fn make_dispatch_ctx<'a>(
        args: &'a mut serde_json::Value,
        state: &'a crate::SessionState,
    ) -> ToolDispatchContext<'a> {
        ToolDispatchContext {
            tool_name: "test_tool",
            tool_call_id: "id1",
            arguments: args,
            state,
        }
    }

    // ── T006: PolicyVerdict and PreDispatchVerdict debug + PolicyContext construction ──

    #[test]
    fn policy_verdict_debug() {
        let v = PolicyVerdict::Continue;
        assert!(format!("{v:?}").contains("Continue"));

        let v = PolicyVerdict::Stop("budget exceeded".to_string());
        assert!(format!("{v:?}").contains("budget exceeded"));

        let v = PolicyVerdict::Inject(vec![]);
        assert!(format!("{v:?}").contains("Inject"));
    }

    #[test]
    fn pre_dispatch_verdict_debug() {
        let v = PreDispatchVerdict::Skip("denied".to_string());
        assert!(format!("{v:?}").contains("denied"));

        let v = PreDispatchVerdict::Stop("halt".to_string());
        assert!(format!("{v:?}").contains("halt"));
    }

    #[test]
    fn policy_context_construction() {
        let (usage, cost) = test_context();
        let state = crate::SessionState::new();
        let ctx = make_ctx(&usage, &cost, &state);
        assert_eq!(ctx.turn_index, 0);
        assert_eq!(ctx.message_count, 5);
        assert!(!ctx.overflow_signal);
    }

    // ── T007: run_policies tests ──

    #[test]
    fn empty_vec_returns_continue() {
        let policies: Vec<Arc<dyn PreTurnPolicy>> = vec![];
        let (usage, cost) = test_context();
        let state = crate::SessionState::new();
        let ctx = make_ctx(&usage, &cost, &state);
        let result = run_policies(&policies, &ctx);
        assert!(matches!(result, PolicyVerdict::Continue));
    }

    #[test]
    fn single_continue() {
        let p = Arc::new(TestPolicy::new("a", || PolicyVerdict::Continue));
        let policies: Vec<Arc<dyn PreTurnPolicy>> = vec![p.clone()];
        let (usage, cost) = test_context();
        let state = crate::SessionState::new();
        let ctx = make_ctx(&usage, &cost, &state);
        let result = run_policies(&policies, &ctx);
        assert!(matches!(result, PolicyVerdict::Continue));
        assert_eq!(p.calls(), 1);
    }

    #[test]
    fn single_stop_short_circuits() {
        let p1 = Arc::new(TestPolicy::new("stopper", || {
            PolicyVerdict::Stop("done".into())
        }));
        let p2 = Arc::new(TestPolicy::new("never_called", || PolicyVerdict::Continue));
        let policies: Vec<Arc<dyn PreTurnPolicy>> = vec![p1.clone(), p2.clone()];
        let (usage, cost) = test_context();
        let state = crate::SessionState::new();
        let ctx = make_ctx(&usage, &cost, &state);
        let result = run_policies(&policies, &ctx);
        assert!(matches!(result, PolicyVerdict::Stop(ref r) if r == "done"));
        assert_eq!(p1.calls(), 1);
        assert_eq!(p2.calls(), 0);
    }

    #[test]
    fn inject_accumulates_across_policies() {
        let p1 = Arc::new(TestPolicy::new("a", || {
            PolicyVerdict::Inject(vec![test_message()])
        }));
        let p2 = Arc::new(TestPolicy::new("b", || {
            PolicyVerdict::Inject(vec![test_message()])
        }));
        let policies: Vec<Arc<dyn PreTurnPolicy>> = vec![p1, p2];
        let (usage, cost) = test_context();
        let state = crate::SessionState::new();
        let ctx = make_ctx(&usage, &cost, &state);
        let result = run_policies(&policies, &ctx);
        match result {
            PolicyVerdict::Inject(msgs) => assert_eq!(msgs.len(), 2),
            _ => panic!("expected Inject"),
        }
    }

    #[test]
    fn stop_after_inject_returns_stop() {
        let p1 = Arc::new(TestPolicy::new("injector", || {
            PolicyVerdict::Inject(vec![test_message()])
        }));
        let p2 = Arc::new(TestPolicy::new("stopper", || {
            PolicyVerdict::Stop("halt".into())
        }));
        let policies: Vec<Arc<dyn PreTurnPolicy>> = vec![p1, p2];
        let (usage, cost) = test_context();
        let state = crate::SessionState::new();
        let ctx = make_ctx(&usage, &cost, &state);
        let result = run_policies(&policies, &ctx);
        assert!(matches!(result, PolicyVerdict::Stop(ref r) if r == "halt"));
    }

    #[test]
    fn panic_caught_returns_continue() {
        let p1: Arc<dyn PreTurnPolicy> = Arc::new(PanickingPolicy);
        let p2 = Arc::new(TestPolicy::new("after_panic", || PolicyVerdict::Continue));
        let policies: Vec<Arc<dyn PreTurnPolicy>> = vec![p1, p2.clone()];
        let (usage, cost) = test_context();
        let state = crate::SessionState::new();
        let ctx = make_ctx(&usage, &cost, &state);
        let result = run_policies(&policies, &ctx);
        assert!(matches!(result, PolicyVerdict::Continue));
        assert_eq!(p2.calls(), 1); // panicking policy skipped, next one runs
    }

    // ── T008: run_pre_dispatch_policies tests ──

    #[test]
    fn pre_dispatch_empty_vec_returns_continue() {
        let policies: Vec<Arc<dyn PreDispatchPolicy>> = vec![];
        let state = crate::SessionState::new();
        let mut args = serde_json::json!({});
        let mut ctx = make_dispatch_ctx(&mut args, &state);
        let result = run_pre_dispatch_policies(&policies, &mut ctx);
        assert!(matches!(result, PreDispatchVerdict::Continue));
    }

    #[test]
    fn pre_dispatch_skip_short_circuits() {
        let p1 = Arc::new(TestPreDispatchPolicy::new("skipper", || {
            PreDispatchVerdict::Skip("denied".into())
        }));
        let p2 = Arc::new(TestPreDispatchPolicy::new("never", || {
            PreDispatchVerdict::Continue
        }));
        let policies: Vec<Arc<dyn PreDispatchPolicy>> = vec![p1.clone(), p2.clone()];
        let state = crate::SessionState::new();
        let mut args = serde_json::json!({});
        let mut ctx = make_dispatch_ctx(&mut args, &state);
        let result = run_pre_dispatch_policies(&policies, &mut ctx);
        assert!(matches!(result, PreDispatchVerdict::Skip(ref e) if e == "denied"));
        assert_eq!(p1.calls(), 1);
        assert_eq!(p2.calls(), 0);
    }

    #[test]
    fn pre_dispatch_stop_short_circuits() {
        let p1 = Arc::new(TestPreDispatchPolicy::new("stopper", || {
            PreDispatchVerdict::Stop("halt".into())
        }));
        let p2 = Arc::new(TestPreDispatchPolicy::new("never", || {
            PreDispatchVerdict::Continue
        }));
        let policies: Vec<Arc<dyn PreDispatchPolicy>> = vec![p1, p2.clone()];
        let state = crate::SessionState::new();
        let mut args = serde_json::json!({});
        let mut ctx = make_dispatch_ctx(&mut args, &state);
        let result = run_pre_dispatch_policies(&policies, &mut ctx);
        assert!(matches!(result, PreDispatchVerdict::Stop(ref r) if r == "halt"));
        assert_eq!(p2.calls(), 0);
    }

    #[test]
    fn pre_dispatch_inject_accumulates() {
        let p1 = Arc::new(TestPreDispatchPolicy::new("a", || {
            PreDispatchVerdict::Inject(vec![test_message()])
        }));
        let p2 = Arc::new(TestPreDispatchPolicy::new("b", || {
            PreDispatchVerdict::Inject(vec![test_message()])
        }));
        let policies: Vec<Arc<dyn PreDispatchPolicy>> = vec![p1, p2];
        let state = crate::SessionState::new();
        let mut args = serde_json::json!({});
        let mut ctx = make_dispatch_ctx(&mut args, &state);
        let result = run_pre_dispatch_policies(&policies, &mut ctx);
        match result {
            PreDispatchVerdict::Inject(msgs) => assert_eq!(msgs.len(), 2),
            _ => panic!("expected Inject"),
        }
    }

    #[test]
    fn pre_dispatch_panic_caught_returns_continue() {
        let p1: Arc<dyn PreDispatchPolicy> = Arc::new(PanickingPreDispatchPolicy);
        let p2 = Arc::new(TestPreDispatchPolicy::new("after", || {
            PreDispatchVerdict::Continue
        }));
        let policies: Vec<Arc<dyn PreDispatchPolicy>> = vec![p1, p2.clone()];
        let state = crate::SessionState::new();
        let mut args = serde_json::json!({});
        let mut ctx = make_dispatch_ctx(&mut args, &state);
        let result = run_pre_dispatch_policies(&policies, &mut ctx);
        assert!(matches!(result, PreDispatchVerdict::Continue));
        assert_eq!(p2.calls(), 1);
    }

    #[test]
    fn argument_mutation_visible_to_next_policy() {
        let mutator: Arc<dyn PreDispatchPolicy> = Arc::new(MutatingPreDispatchPolicy);
        let verifier: Arc<dyn PreDispatchPolicy> = Arc::new(VerifyingPreDispatchPolicy {
            expected_key: "injected".to_string(),
        });
        let policies: Vec<Arc<dyn PreDispatchPolicy>> = vec![mutator, verifier];
        let state = crate::SessionState::new();
        let mut args = serde_json::json!({"original": "value"});
        let mut ctx = make_dispatch_ctx(&mut args, &state);
        let result = run_pre_dispatch_policies(&policies, &mut ctx);
        // If mutator didn't inject "injected" key, verifier would return Skip
        assert!(matches!(result, PreDispatchVerdict::Continue));
        // Verify the mutation is visible in the original args after dispatch
        assert_eq!(args["injected"], "by_policy");
    }

    #[test]
    fn tool_dispatch_context_contains_only_reliable_fields() {
        // Regression: ToolDispatchContext must not include loop-level metrics
        // (turn_index, usage, cost, message_count, overflow_signal, new_messages)
        // because those are not tracked at the tool dispatch call site.
        let state = crate::SessionState::new();
        let mut args = serde_json::json!({"path": "/tmp/file"});
        let ctx = ToolDispatchContext {
            tool_name: "write_file",
            tool_call_id: "call-123",
            arguments: &mut args,
            state: &state,
        };
        assert_eq!(ctx.tool_name, "write_file");
        assert_eq!(ctx.tool_call_id, "call-123");
        assert_eq!(ctx.arguments["path"], "/tmp/file");
        // Debug output does not expose argument values
        let debug_str = format!("{ctx:?}");
        assert!(debug_str.contains("write_file"));
        assert!(!debug_str.contains("/tmp/file"), "arguments must be redacted in Debug");
    }
}
