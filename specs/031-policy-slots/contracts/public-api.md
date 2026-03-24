# Public API Contract: Configurable Policy Slots

**Feature**: 031-policy-slots | **Date**: 2026-03-24

## Verdict Enums

```rust
/// Outcome for PreTurn, PostTurn, and PostLoop slots.
#[derive(Debug, Clone)]
pub enum PolicyVerdict {
    Continue,
    Stop(String),
    Inject(Vec<AgentMessage>),
}

/// Outcome for PreDispatch slot (includes Skip).
#[derive(Debug, Clone)]
pub enum PreDispatchVerdict {
    Continue,
    Stop(String),
    Inject(Vec<AgentMessage>),
    Skip(String),
}
```

## Context Structs

```rust
/// Shared read-only context for all policy evaluations.
#[derive(Debug)]
pub struct PolicyContext<'a> {
    pub turn_index: usize,
    pub accumulated_usage: &'a Usage,
    pub accumulated_cost: &'a Cost,
    pub message_count: usize,
    pub overflow_signal: bool,
}

/// Per-tool-call context for PreDispatch policies.
pub struct ToolPolicyContext<'a> {
    pub tool_name: &'a str,
    pub tool_call_id: &'a str,
    pub arguments: &'a mut Value,
}

/// Per-turn context for PostTurn policies.
#[derive(Debug)]
pub struct TurnPolicyContext<'a> {
    pub assistant_message: &'a AssistantMessage,
    pub tool_results: &'a [ToolResultMessage],
    pub stop_reason: StopReason,
}
```

## Slot Traits

```rust
/// Slot 1: Before each LLM call.
pub trait PreTurnPolicy: Send + Sync {
    fn name(&self) -> &str;
    fn evaluate(&self, ctx: &PolicyContext<'_>) -> PolicyVerdict;
}

/// Slot 2: Per tool call, before approval and execution.
pub trait PreDispatchPolicy: Send + Sync {
    fn name(&self) -> &str;
    fn evaluate(
        &self,
        ctx: &PolicyContext<'_>,
        tool: &mut ToolPolicyContext<'_>,
    ) -> PreDispatchVerdict;
}

/// Slot 3: After each completed turn.
pub trait PostTurnPolicy: Send + Sync {
    fn name(&self) -> &str;
    fn evaluate(
        &self,
        ctx: &PolicyContext<'_>,
        turn: &TurnPolicyContext<'_>,
    ) -> PolicyVerdict;
}

/// Slot 4: After inner loop exits, before follow-up polling.
pub trait PostLoopPolicy: Send + Sync {
    fn name(&self) -> &str;
    fn evaluate(&self, ctx: &PolicyContext<'_>) -> PolicyVerdict;
}
```

## Slot Runner Functions

```rust
/// Evaluate PreTurn/PostTurn/PostLoop policies in order.
/// Stop short-circuits. Inject accumulates. Panics caught as Continue.
pub fn run_policies<P>(policies: &[Arc<P>], ...) -> PolicyVerdict;

/// Evaluate PreDispatch policies in order.
/// Stop/Skip short-circuit. Inject accumulates. Panics caught as Continue.
pub fn run_pre_dispatch_policies(
    policies: &[Arc<dyn PreDispatchPolicy>],
    ctx: &PolicyContext<'_>,
    tool: &mut ToolPolicyContext<'_>,
) -> PreDispatchVerdict;
```

## Built-in Policies

```rust
// --- PreTurn ---

pub struct BudgetPolicy {
    pub max_cost: Option<f64>,
    pub max_input_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
}

impl BudgetPolicy {
    pub fn new() -> Self;                                  // no limits
    pub fn max_cost(mut self, limit: f64) -> Self;
    pub fn max_input_tokens(mut self, limit: u64) -> Self;
    pub fn max_output_tokens(mut self, limit: u64) -> Self;
}
impl PreTurnPolicy for BudgetPolicy { ... }

pub struct MaxTurnsPolicy {
    pub max_turns: usize,
}

impl MaxTurnsPolicy {
    pub fn new(max_turns: usize) -> Self;
}
impl PreTurnPolicy for MaxTurnsPolicy { ... }
impl PostTurnPolicy for MaxTurnsPolicy { ... }

// --- PreDispatch ---

pub struct SandboxPolicy { /* allowed_root: PathBuf, path_fields: Vec<String> */ }

impl SandboxPolicy {
    pub fn new(allowed_root: impl Into<PathBuf>) -> Self;  // default path_fields: ["path", "file_path", "file"]
    pub fn with_path_fields(mut self, fields: impl IntoIterator<Item = impl Into<String>>) -> Self;
}
impl PreDispatchPolicy for SandboxPolicy { ... }  // Skip with error on violation (no silent rewrite)

pub struct ToolDenyListPolicy { /* denied: HashSet<String> */ }

impl ToolDenyListPolicy {
    pub fn new(denied: impl IntoIterator<Item = impl Into<String>>) -> Self;
}
impl PreDispatchPolicy for ToolDenyListPolicy { ... }

// --- PostTurn ---

pub struct CheckpointPolicy { /* store: Arc<dyn CheckpointStore>, handle: Handle */ }

impl CheckpointPolicy {
    pub fn new(store: Arc<dyn CheckpointStore>) -> Self;  // captures Handle::current()
    pub fn with_handle(mut self, handle: tokio::runtime::Handle) -> Self;
}
impl PostTurnPolicy for CheckpointPolicy { ... }  // fire-and-forget via tokio::spawn, returns Continue

pub struct LoopDetectionPolicy { /* lookback, on_detect, history */ }

impl LoopDetectionPolicy {
    pub fn new(lookback: usize) -> Self;           // default: Stop on detect
    pub fn with_steering(mut self, message: impl Into<String>) -> Self; // Inject instead
}
impl PostTurnPolicy for LoopDetectionPolicy { ... }
```

## AgentOptions Builder Methods

```rust
// Policy slot builders (each pushes to the respective vec)
.with_pre_turn_policy(policy: impl PreTurnPolicy + 'static) -> Self
.with_pre_dispatch_policy(policy: impl PreDispatchPolicy + 'static) -> Self
.with_post_turn_policy(policy: impl PostTurnPolicy + 'static) -> Self
.with_post_loop_policy(policy: impl PostLoopPolicy + 'static) -> Self

// Removed (replaced by policy slot equivalents):
// .with_budget_guard(guard)
// .with_cost_limit(max_cost)
// .with_token_limit(max_tokens)
// .with_loop_policy(policy)
// .with_post_turn_hook(hook)
// .with_tool_validator(validator)
// .with_tool_call_transformer(transformer)
```

## Re-exports from `lib.rs`

```rust
// New policy slot system
pub use policy::{
    PolicyVerdict, PreDispatchVerdict,
    PolicyContext, ToolPolicyContext, TurnPolicyContext,
    PreTurnPolicy, PreDispatchPolicy, PostTurnPolicy, PostLoopPolicy,
    run_policies, run_pre_dispatch_policies,
};
pub use policies::{
    BudgetPolicy, MaxTurnsPolicy,
    SandboxPolicy, ToolDenyListPolicy,
    CheckpointPolicy, LoopDetectionPolicy,
    LoopDetectionAction,
};

// Removed:
// pub use budget_guard::{BudgetGuard, BudgetExceeded};
// pub use loop_policy::{LoopPolicy, PolicyContext as OldPolicyContext, ...};
// pub use post_turn_hook::{PostTurnHook, PostTurnContext, PostTurnAction};
// pub use tool_validator::ToolValidator;
// pub use tool_call_transformer::ToolCallTransformer;
```
