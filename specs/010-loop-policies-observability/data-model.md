# Data Model: Loop Policies & Observability

**Feature**: 010-loop-policies-observability | **Date**: 2026-03-20

> **Partially superseded by [031-policy-slots](../031-policy-slots/spec.md).**
> LoopPolicy, PolicyContext, MaxTurnsPolicy, CostCapPolicy, ComposedPolicy, PostTurnHook, PostTurnAction, PostTurnContext, BudgetGuard, and BudgetExceeded are all replaced by the four-slot policy system. See 031 for the new PolicyVerdict, PolicyContext, ToolPolicyContext, TurnPolicyContext, and slot traits.
> StreamMiddleware, MetricsCollector, TurnMetrics, ToolExecMetrics, Checkpoint, LoopCheckpoint, and CheckpointStore remain valid.

## Entities

### PolicyContext (superseded by 031)

Snapshot of loop state provided to policy decisions at turn boundaries.

| Field | Type | Description |
|-------|------|-------------|
| `turn_index` | `usize` | Zero-based index of the completed turn |
| `accumulated_usage` | `Usage` | Accumulated token usage across all turns |
| `accumulated_cost` | `Cost` | Accumulated cost across all turns |
| `assistant_message` | `&AssistantMessage` | The assistant message from the just-completed turn |
| `stop_reason` | `StopReason` | The stop reason from the just-completed turn |

Implements: `Debug`. Lifetime: borrows from the loop's turn data.

### LoopPolicy (trait) — superseded by 031 PostTurnPolicy

Controls whether the agent loop continues after each turn.

```rust
pub trait LoopPolicy: Send + Sync {
    fn should_continue(&self, ctx: &PolicyContext<'_>) -> bool;
}
```

Blanket impl: `impl<F: Fn(&PolicyContext) -> bool + Send + Sync> LoopPolicy for F`.

### MaxTurnsPolicy

Limits the agent loop to a maximum number of turns.

| Field | Type | Description |
|-------|------|-------------|
| `max_turns` | `usize` | Maximum number of turns before stopping |

Implements: `LoopPolicy`, `Debug`, `Clone`.

### CostCapPolicy

Limits the agent loop by total accumulated cost.

| Field | Type | Description |
|-------|------|-------------|
| `max_cost` | `f64` | Maximum total cost before stopping |

Implements: `LoopPolicy`, `Debug`, `Clone`.

### ComposedPolicy

Composes multiple policies with AND semantics (all must return `true` to continue).

| Field | Type | Description |
|-------|------|-------------|
| `policies` | `Vec<Box<dyn LoopPolicy>>` | Inner policies evaluated in order |

Implements: `LoopPolicy`, `Debug` (shows policy count).

### StreamMiddleware

Decorator wrapping a `StreamFn` to intercept/transform the output event stream.

| Field | Type | Description |
|-------|------|-------------|
| `inner` | `Arc<dyn StreamFn>` | The wrapped stream function |
| `map_stream` | `MapStreamFn` | Closure that transforms the inner stream |

Implements: `StreamFn`, `Debug`.

`MapStreamFn` type alias:
```rust
type MapStreamFn = Arc<
    dyn for<'a> Fn(
        Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>>,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>>
    + Send + Sync,
>;
```

### Emission

A structured event emitted by an agent, tool, or callback.

| Field | Type | Description |
|-------|------|-------------|
| `name` | `String` | Event name (e.g., "progress", "artifact_created") |
| `payload` | `Value` | Structured JSON payload |

Implements: `Debug`, `Clone`.

### TurnMetrics

Metrics snapshot emitted at the end of each agent loop turn.

| Field | Type | Description |
|-------|------|-------------|
| `turn_index` | `usize` | Zero-based index of the turn |
| `llm_call_duration` | `Duration` | Wall-clock duration of the LLM streaming call |
| `tool_executions` | `Vec<ToolExecMetrics>` | Per-tool execution metrics |
| `usage` | `Usage` | Token usage for this turn |
| `cost` | `Cost` | Cost attributed to this turn |
| `turn_duration` | `Duration` | Total wall-clock duration (LLM + tools) |

Implements: `Debug`, `Clone`, `Serialize`, `Deserialize`.

### ToolExecMetrics

Timing and outcome data for a single tool execution within a turn.

| Field | Type | Description |
|-------|------|-------------|
| `tool_name` | `String` | Name of the executed tool |
| `duration` | `Duration` | Wall-clock duration of the execution |
| `success` | `bool` | Whether the execution succeeded |

Implements: `Debug`, `Clone`, `Serialize`, `Deserialize`.

### MetricsCollector (trait)

Async observer that receives structured metrics at the end of each turn.

```rust
pub trait MetricsCollector: Send + Sync {
    fn on_metrics<'a>(
        &'a self,
        metrics: &'a TurnMetrics,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}
```

### PostTurnContext — superseded by 031 TurnPolicyContext

Snapshot of state provided to the post-turn hook.

| Field | Type | Description |
|-------|------|-------------|
| `turn_index` | `usize` | Zero-based index of the completed turn |
| `assistant_message` | `&AssistantMessage` | The assistant message from this turn |
| `tool_results` | `&[ToolResultMessage]` | Tool results produced during this turn |
| `accumulated_usage` | `&Usage` | Accumulated token usage |
| `accumulated_cost` | `&Cost` | Accumulated cost |
| `messages` | `&[AgentMessage]` | Full conversation history at this point |

Implements: `Debug`. Lifetime: borrows from the loop's turn data.

### PostTurnAction — superseded by 031 PolicyVerdict

Action returned by a PostTurnHook to influence loop behavior.

| Variant | Fields | Description |
|---------|--------|-------------|
| `Continue` | — | Continue the loop normally |
| `Stop` | `Option<String>` | Stop the loop with an optional reason |
| `InjectMessages` | `Vec<AgentMessage>` | Inject messages before the next turn |

Implements: `Debug`.

### PostTurnHook (trait) — superseded by 031 PostTurnPolicy

Hook invoked after each completed turn in the agent loop.

```rust
pub trait PostTurnHook: Send + Sync {
    fn on_turn_end<'a>(
        &'a self,
        ctx: &'a PostTurnContext<'a>,
    ) -> Pin<Box<dyn Future<Output = PostTurnAction> + Send + 'a>>;
}
```

### BudgetGuard — superseded by 031 BudgetPolicy (PreTurnPolicy)

Pre-call budget limits that prevent an LLM call from starting when accumulated cost or token usage has exceeded the budget.

| Field | Type | Description |
|-------|------|-------------|
| `max_cost` | `Option<f64>` | Maximum total cost before blocking |
| `max_tokens` | `Option<u64>` | Maximum total tokens before blocking |

Implements: `Debug`, `Clone`, `Default`.

### BudgetExceeded

Describes which budget limit was exceeded.

| Variant | Fields | Description |
|---------|--------|-------------|
| `Cost` | `limit: f64, actual: f64` | Cost limit exceeded |
| `Tokens` | `limit: u64, actual: u64` | Token limit exceeded |

Implements: `Debug`, `Clone`, `PartialEq`, `Display`.

### Checkpoint

A serializable snapshot of agent conversation state.

| Field | Type | Description |
|-------|------|-------------|
| `id` | `String` | Unique identifier |
| `system_prompt` | `String` | System prompt at checkpoint time |
| `provider` | `String` | Model provider name |
| `model_id` | `String` | Model identifier |
| `messages` | `Vec<LlmMessage>` | Conversation messages (CustomMessage filtered out) |
| `turn_count` | `usize` | Number of completed turns |
| `usage` | `Usage` | Accumulated token usage |
| `cost` | `Cost` | Accumulated cost |
| `created_at` | `u64` | Unix timestamp |
| `metadata` | `HashMap<String, Value>` | Arbitrary application-specific metadata |

Implements: `Debug`, `Clone`, `Serialize`, `Deserialize`.

### LoopCheckpoint

A serializable snapshot of the agent loop's in-flight state for pause/resume.

| Field | Type | Description |
|-------|------|-------------|
| `messages` | `Vec<LlmMessage>` | All context messages at pause time |
| `pending_messages` | `Vec<LlmMessage>` | Messages queued for next turn |
| `overflow_signal` | `bool` | Whether context overflow was active |
| `turn_index` | `usize` | Zero-based turn index at pause time |
| `usage` | `Usage` | Accumulated token usage |
| `cost` | `Cost` | Accumulated cost |
| `system_prompt` | `String` | Active system prompt |
| `provider` | `String` | Model provider name |
| `model_id` | `String` | Model identifier |
| `last_assistant_message` | `Option<AssistantMessage>` | Last assistant message for continuity |
| `created_at` | `u64` | Unix timestamp |
| `metadata` | `HashMap<String, Value>` | Arbitrary metadata |

Implements: `Debug`, `Clone`, `Serialize`, `Deserialize`.

### CheckpointStore (trait)

Async trait for persisting and loading agent checkpoints.

```rust
pub trait CheckpointStore: Send + Sync {
    fn save_checkpoint(&self, checkpoint: &Checkpoint) -> AsyncResult<'_, ()>;
    fn load_checkpoint(&self, id: &str) -> AsyncResult<'_, Option<Checkpoint>>;
    fn list_checkpoints(&self) -> AsyncResult<'_, Vec<String>>;
    fn delete_checkpoint(&self, id: &str) -> AsyncResult<'_, ()>;
}
```

Where `AsyncResult<'a, T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send + 'a>>`.

### OTel Span Hierarchy (feature-gated: `otel`)

The OpenTelemetry integration maps the agent loop lifecycle to a span tree. No new structs are introduced — the integration is implemented via `tracing` instrumentation spans with recorded fields.

**Span tree**:

| Span Name | Parent | Lifetime | Key Attributes |
|-----------|--------|----------|----------------|
| `agent.run` | (root or caller span) | Full `agent_loop` / `agent_loop_continue` call | — |
| `agent.turn` | `agent.run` | Single turn (TurnStart → TurnEnd) | `agent.turn_index`, `agent.stop_reason` |
| `agent.llm_call` | `agent.turn` | LLM streaming call duration | `agent.model`, `agent.tokens.input`, `agent.tokens.output`, `agent.cost.total` |
| `agent.tool.{name}` | `agent.turn` | Tool execution duration | `agent.tool.name`, `otel.status_code` (OK/ERROR) |

**Semantic attributes** (recorded as `tracing` span fields):

| Attribute | Type | Source |
|-----------|------|--------|
| `agent.model` | `String` | `ModelSpec.model_id` from `AgentLoopConfig` / `BeforeLlmCall` event |
| `agent.turn_index` | `u64` | `TurnMetrics.turn_index` or loop counter |
| `agent.tokens.input` | `u64` | `TurnMetrics.usage.input` |
| `agent.tokens.output` | `u64` | `TurnMetrics.usage.output` |
| `agent.cost.total` | `f64` | `TurnMetrics.cost.total` |
| `agent.tool.name` | `String` | `ToolExecutionStart.name` |
| `agent.stop_reason` | `String` | `TurnEndReason` display value |

**Integration point**: `tracing::info_span!` calls in `src/loop_/mod.rs` and `src/loop_/turn.rs`, gated by `#[cfg(feature = "otel")]`. When the feature is disabled, these are standard `tracing` spans (already present for diagnostics). When enabled, `tracing-opentelemetry` bridges them to OTel.

### OtelInitConfig (feature-gated: `otel`)

Configuration for the convenience `init_otel_layer()` function.

| Field | Type | Description |
|-------|------|-------------|
| `service_name` | `String` | OTel service name (default: `"swink-agent"`) |
| `endpoint` | `Option<String>` | OTLP exporter endpoint (default: `http://localhost:4317` for gRPC) |

Implements: `Debug`, `Clone`, `Default`.

## Relationships

```
LoopPolicy <-- MaxTurnsPolicy, CostCapPolicy, ComposedPolicy, closures  [SUPERSEDED by 031 policy slots]
    PolicyContext --reads--> Usage, Cost, AssistantMessage, StopReason  [SUPERSEDED by 031 PolicyContext]

StreamMiddleware --wraps--> Arc<dyn StreamFn>
    StreamMiddleware --implements--> StreamFn
    StreamMiddleware --transforms--> Stream<Item = AssistantMessageEvent>

MetricsCollector --receives--> TurnMetrics
    TurnMetrics --contains--> Vec<ToolExecMetrics>
    TurnMetrics --contains--> Usage, Cost

PostTurnHook --receives--> PostTurnContext  [SUPERSEDED by 031 PostTurnPolicy + TurnPolicyContext]
    PostTurnHook --returns--> PostTurnAction  [SUPERSEDED by 031 PolicyVerdict]
    PostTurnContext --borrows--> AssistantMessage, ToolResultMessage, Usage, Cost, AgentMessage

BudgetGuard --checks--> Usage, Cost  [SUPERSEDED by 031 BudgetPolicy in PreTurn slot]
    BudgetGuard --returns--> Result<(), BudgetExceeded>

Checkpoint --contains--> Vec<LlmMessage>, Usage, Cost
LoopCheckpoint --contains--> Vec<LlmMessage>, Usage, Cost, AssistantMessage
    LoopCheckpoint --converts-to--> Checkpoint (via to_checkpoint)
CheckpointStore --persists--> Checkpoint

OTel Span Hierarchy (feature = "otel"):
    agent.run --parent-of--> agent.turn
    agent.turn --parent-of--> agent.llm_call
    agent.turn --parent-of--> agent.tool.{name}
    agent.llm_call --records--> ModelSpec, Usage, Cost
    agent.tool.{name} --records--> tool_name, duration, success
    tracing-opentelemetry --bridges--> tracing spans to OTel SDK
    OtelInitConfig --configures--> TracerProvider (OTLP exporter)
```
