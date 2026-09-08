//! Tests for `policy`.
#![cfg(test)]

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
        execution_root: None,
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
        execution_root: None,
        state: &state,
    };
    assert_eq!(ctx.tool_name, "write_file");
    assert_eq!(ctx.tool_call_id, "call-123");
    assert_eq!(ctx.arguments["path"], "/tmp/file");
    // Debug output does not expose argument values
    let debug_str = format!("{ctx:?}");
    assert!(debug_str.contains("write_file"));
    assert!(
        !debug_str.contains("/tmp/file"),
        "arguments must be redacted in Debug"
    );
}
