# Public API Contract: Loop Policies & Observability

**Feature**: 010-loop-policies-observability | **Date**: 2026-03-20

## LoopPolicy (trait)

```rust
pub trait LoopPolicy: Send + Sync {
    fn should_continue(&self, ctx: &PolicyContext<'_>) -> bool;
}

// Blanket impl for closures
impl<F: Fn(&PolicyContext<'_>) -> bool + Send + Sync> LoopPolicy for F
```

## PolicyContext

```rust
pub struct PolicyContext<'a> {
    pub turn_index: usize,
    pub accumulated_usage: Usage,
    pub accumulated_cost: Cost,
    pub assistant_message: &'a AssistantMessage,
    pub stop_reason: StopReason,
}
```

## MaxTurnsPolicy

```rust
// Constructor
MaxTurnsPolicy::new(max_turns: usize) -> MaxTurnsPolicy     // const fn

// LoopPolicy impl
policy.should_continue(ctx) -> bool                          // true when turn_index < max_turns
```

## CostCapPolicy

```rust
// Constructor
CostCapPolicy::new(max_cost: f64) -> CostCapPolicy          // const fn

// LoopPolicy impl
policy.should_continue(ctx) -> bool                          // true when accumulated_cost.total <= max_cost
```

## ComposedPolicy

```rust
// Constructor
ComposedPolicy::new(policies: Vec<Box<dyn LoopPolicy>>) -> ComposedPolicy

// LoopPolicy impl (AND semantics)
policy.should_continue(ctx) -> bool                          // true when all inner policies return true
```

## StreamMiddleware

```rust
// Full stream transformation
StreamMiddleware::new(inner: Arc<dyn StreamFn>, f: impl Fn(Stream) -> Stream) -> StreamMiddleware

// Convenience constructors
StreamMiddleware::with_logging(inner, callback: impl Fn(&AssistantMessageEvent)) -> StreamMiddleware
StreamMiddleware::with_map(inner, f: impl Fn(AssistantMessageEvent) -> AssistantMessageEvent) -> StreamMiddleware
StreamMiddleware::with_filter(inner, f: impl Fn(&AssistantMessageEvent) -> bool) -> StreamMiddleware

// Implements StreamFn — transparent to consumers
mw.stream(model, context, options, cancellation_token) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent>>>
```

## Emission

```rust
// Constructor
Emission::new(name: impl Into<String>, payload: Value) -> Emission

// Fields (pub)
emission.name: String
emission.payload: Value
```

## TurnMetrics

```rust
// Fields (pub)
metrics.turn_index: usize
metrics.llm_call_duration: Duration
metrics.tool_executions: Vec<ToolExecMetrics>
metrics.usage: Usage
metrics.cost: Cost
metrics.turn_duration: Duration
```

Derives: `Debug`, `Clone`, `Serialize`, `Deserialize`.

## ToolExecMetrics

```rust
// Fields (pub)
metrics.tool_name: String
metrics.duration: Duration
metrics.success: bool
```

Derives: `Debug`, `Clone`, `Serialize`, `Deserialize`.

## MetricsCollector (trait)

```rust
pub trait MetricsCollector: Send + Sync {
    fn on_metrics<'a>(
        &'a self,
        metrics: &'a TurnMetrics,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}
```

## PostTurnContext

```rust
pub struct PostTurnContext<'a> {
    pub turn_index: usize,
    pub assistant_message: &'a AssistantMessage,
    pub tool_results: &'a [ToolResultMessage],
    pub accumulated_usage: &'a Usage,
    pub accumulated_cost: &'a Cost,
    pub messages: &'a [AgentMessage],
}
```

## PostTurnAction

```rust
pub enum PostTurnAction {
    Continue,
    Stop(Option<String>),
    InjectMessages(Vec<AgentMessage>),
}
```

## PostTurnHook (trait)

```rust
pub trait PostTurnHook: Send + Sync {
    fn on_turn_end<'a>(
        &'a self,
        ctx: &'a PostTurnContext<'a>,
    ) -> Pin<Box<dyn Future<Output = PostTurnAction> + Send + 'a>>;
}
```

## BudgetGuard

```rust
// Constructor
BudgetGuard::new() -> BudgetGuard                           // const fn, no limits
BudgetGuard::default() -> BudgetGuard                       // same as new()

// Builder methods (all const fn, return Self)
.with_max_cost(max_cost: f64) -> Self
.with_max_tokens(max_tokens: u64) -> Self

// Check
guard.check(usage: &Usage, cost: &Cost) -> Result<(), BudgetExceeded>    // const fn

// Fields (pub)
guard.max_cost: Option<f64>
guard.max_tokens: Option<u64>
```

## BudgetExceeded

```rust
pub enum BudgetExceeded {
    Cost { limit: f64, actual: f64 },
    Tokens { limit: u64, actual: u64 },
}
```

Implements: `Debug`, `Clone`, `PartialEq`, `Display`.

## Checkpoint

```rust
// Constructor
Checkpoint::new(id, system_prompt, provider, model_id, messages: &[AgentMessage]) -> Checkpoint

// Builder methods (return Self)
.with_turn_count(turn_count: usize) -> Self           // const fn
.with_usage(usage: Usage) -> Self
.with_cost(cost: Cost) -> Self
.with_metadata(key: impl Into<String>, value: Value) -> Self

// Restore
checkpoint.restore_messages() -> Vec<AgentMessage>
```

Derives: `Debug`, `Clone`, `Serialize`, `Deserialize`.

## LoopCheckpoint

```rust
// Constructor
LoopCheckpoint::new(system_prompt, provider, model_id, messages: &[AgentMessage]) -> LoopCheckpoint

// Builder methods (return Self)
.with_turn_index(turn_index: usize) -> Self            // const fn
.with_usage(usage: Usage) -> Self
.with_cost(cost: Cost) -> Self
.with_pending_messages(pending: Vec<LlmMessage>) -> Self
.with_overflow_signal(signal: bool) -> Self            // const fn
.with_last_assistant_message(msg: AssistantMessage) -> Self
.with_metadata(key: impl Into<String>, value: Value) -> Self

// Restore
loop_cp.restore_messages() -> Vec<AgentMessage>
loop_cp.restore_pending_messages() -> Vec<AgentMessage>

// Conversion
loop_cp.to_checkpoint(id: impl Into<String>) -> Checkpoint
```

Derives: `Debug`, `Clone`, `Serialize`, `Deserialize`.

## CheckpointStore (trait)

```rust
pub trait CheckpointStore: Send + Sync {
    fn save_checkpoint(&self, checkpoint: &Checkpoint) -> AsyncResult<'_, ()>;
    fn load_checkpoint(&self, id: &str) -> AsyncResult<'_, Option<Checkpoint>>;
    fn list_checkpoints(&self) -> AsyncResult<'_, Vec<String>>;
    fn delete_checkpoint(&self, id: &str) -> AsyncResult<'_, ()>;
}
```

Where `AsyncResult<'a, T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send + 'a>>`.

## Re-exports (lib.rs)

All public types are re-exported from `lib.rs`:

```rust
pub use loop_policy::{LoopPolicy, PolicyContext, MaxTurnsPolicy, CostCapPolicy, ComposedPolicy};
pub use stream_middleware::StreamMiddleware;
pub use emit::Emission;
pub use metrics::{MetricsCollector, TurnMetrics, ToolExecMetrics};
pub use post_turn_hook::{PostTurnHook, PostTurnContext, PostTurnAction};
pub use budget_guard::{BudgetGuard, BudgetExceeded};
pub use checkpoint::{Checkpoint, LoopCheckpoint, CheckpointStore};
```
