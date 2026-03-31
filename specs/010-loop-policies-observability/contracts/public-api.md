# Public API Contract: Loop Policies & Observability

**Feature**: 010-loop-policies-observability | **Date**: 2026-03-20

> **Partially superseded by [031-policy-slots](../../031-policy-slots/spec.md).**
> LoopPolicy, PolicyContext, MaxTurnsPolicy, CostCapPolicy, ComposedPolicy, PostTurnHook, PostTurnAction, PostTurnContext, BudgetGuard, and BudgetExceeded are replaced by the four-slot policy system.
> See 031 for the new API contracts. StreamMiddleware, MetricsCollector, Checkpoint, and CheckpointStore APIs remain valid.

## LoopPolicy (trait) — superseded by 031

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

## PostTurnContext — superseded by 031 TurnPolicyContext

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

## PostTurnAction — superseded by 031 PolicyVerdict

```rust
pub enum PostTurnAction {
    Continue,
    Stop(Option<String>),
    InjectMessages(Vec<AgentMessage>),
}
```

## PostTurnHook (trait) — superseded by 031 PostTurnPolicy

```rust
pub trait PostTurnHook: Send + Sync {
    fn on_turn_end<'a>(
        &'a self,
        ctx: &'a PostTurnContext<'a>,
    ) -> Pin<Box<dyn Future<Output = PostTurnAction> + Send + 'a>>;
}
```

## BudgetGuard — superseded by 031 BudgetPolicy (PreTurnPolicy)

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

## BudgetExceeded — superseded by 031 PolicyVerdict::Stop

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
// [031] Superseded — replaced by policy slot traits and built-in policy impls:
// pub use loop_policy::{LoopPolicy, PolicyContext, MaxTurnsPolicy, CostCapPolicy, ComposedPolicy};
// pub use post_turn_hook::{PostTurnHook, PostTurnContext, PostTurnAction};
// pub use budget_guard::{BudgetGuard, BudgetExceeded};

// [031] New policy slot exports (defined and documented in 031-policy-slots):
// pub use policy::{PolicyVerdict, PolicyContext, ToolPolicyContext, TurnPolicyContext};
// pub use policy::{PreTurnPolicy, PreDispatchPolicy, PostTurnPolicy, PostLoopPolicy};
// pub use policy::{BudgetPolicy, MaxTurnsPolicy, SandboxPolicy, ToolDenyListPolicy};
// pub use policy::{CheckpointPolicy, LoopDetectionPolicy};
// See 031-policy-slots spec for the full API contract of these types.

// Unchanged:
pub use stream_middleware::StreamMiddleware;
pub use emit::Emission;
pub use metrics::{MetricsCollector, TurnMetrics, ToolExecMetrics};
pub use checkpoint::{Checkpoint, LoopCheckpoint, CheckpointStore};

// New (feature = "otel"):
#[cfg(feature = "otel")]
pub use otel::{OtelInitConfig, init_otel_layer};
```

## OpenTelemetry Integration (feature = "otel")

### Span Instrumentation (core loop)

The agent loop emits `tracing` spans at key lifecycle points. These are always present as `tracing` spans (useful for `tracing_subscriber::fmt` logging) and become OTel spans when `tracing-opentelemetry` is configured as a subscriber layer.

```rust
// Span hierarchy (created via tracing::info_span! in the loop):
//
// agent.run                              — root span, covers full agent_loop() call
//   └─ agent.turn                        — per turn, fields: agent.turn_index, agent.stop_reason
//       ├─ agent.llm_call                — per LLM streaming call, fields: agent.model,
//       │                                    agent.tokens.input, agent.tokens.output, agent.cost.total
//       ├─ agent.tool.{name}             — per tool execution, fields: agent.tool.name
//       └─ agent.tool.{name}             — concurrent tools are sibling spans
```

**Span field types** (recorded via `tracing::Span::record`):

| Field | Type | When Recorded |
|-------|------|---------------|
| `agent.model` | `&str` | On `agent.llm_call` span entry |
| `agent.turn_index` | `u64` | On `agent.turn` span entry |
| `agent.tokens.input` | `u64` | On `agent.llm_call` span exit (after streaming completes) |
| `agent.tokens.output` | `u64` | On `agent.llm_call` span exit |
| `agent.cost.total` | `f64` | On `agent.llm_call` span exit |
| `agent.tool.name` | `&str` | On `agent.tool.{name}` span entry |
| `agent.stop_reason` | `&str` | On `agent.turn` span exit |

### OtelInitConfig

```rust
#[cfg(feature = "otel")]
pub struct OtelInitConfig {
    pub service_name: String,       // default: "swink-agent"
    pub endpoint: Option<String>,   // default: "http://localhost:4317" (gRPC OTLP)
}
```

Derives: `Debug`, `Clone`, `Default`.

### init_otel_layer

```rust
#[cfg(feature = "otel")]
pub fn init_otel_layer(
    config: OtelInitConfig,
) -> impl tracing_subscriber::Layer<S>
where
    S: tracing::Subscriber + for<'span> tracing_subscriber::registry::LookupSpan<'span>;
```

Returns a configured `tracing_opentelemetry::OpenTelemetryLayer` with an OTLP exporter. Users compose this into their `tracing_subscriber` stack:

```rust
use tracing_subscriber::prelude::*;
use swink_agent::otel::{OtelInitConfig, init_otel_layer};

let otel_layer = init_otel_layer(OtelInitConfig::default());
tracing_subscriber::registry()
    .with(otel_layer)
    .with(tracing_subscriber::fmt::layer()) // optional: also log to console
    .init();
```
