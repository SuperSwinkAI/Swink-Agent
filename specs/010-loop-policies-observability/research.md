# Research: Loop Policies & Observability

**Feature**: 010-loop-policies-observability | **Date**: 2026-03-20

## Design Decisions

### D1: Composable policies via trait + closure blanket impl

**Decision**: `LoopPolicy` is a synchronous trait with a single method `should_continue(&self, ctx: &PolicyContext) -> bool`. A blanket impl covers `Fn(&PolicyContext) -> bool` closures, allowing ad-hoc policies without defining a struct. `ComposedPolicy` holds `Vec<Box<dyn LoopPolicy>>` with AND semantics — all policies must return `true` for the loop to continue.

**Rationale**: Synchronous evaluation is correct because policies operate on already-computed turn data (`PolicyContext` snapshot). No I/O or async work is needed to decide "should I continue?" The blanket closure impl removes ceremony for simple one-off rules (e.g., `|ctx| ctx.turn_index < 10`). AND composition means any single policy can stop the loop, which is the natural safety model — you want the most restrictive policy to win.

**Alternatives rejected**:
- **Async policy trait**: Would add unnecessary complexity. Policy decisions are pure predicates over in-memory data. Any async work (e.g., checking an external budget service) belongs in a PostTurnHook, not a policy.
- **OR composition**: Would require all policies to agree on stopping, making it easy for a permissive policy to override a restrictive one. AND semantics are safer for governance.
- **Priority-based policies**: Adds ordering complexity. AND semantics are simpler and sufficient — if you need priority, compose at the application level.

### D2: Async PostTurnHook with PostTurnAction enum

**Decision**: `PostTurnHook` is an async trait returning `PostTurnAction` (Continue/Stop/InjectMessages). The hook receives `PostTurnContext` containing turn index, assistant message, tool results, accumulated usage/cost, and the full message history. Panicking hooks are caught via `catch_unwind`, logged, and skipped — the loop continues.

**Rationale**: Hooks need async because their primary use cases (persisting state, flushing metrics, calling external APIs) involve I/O. The `PostTurnAction` enum gives hooks explicit control over loop behavior: `Continue` for observe-only hooks, `Stop` for budget enforcement, `InjectMessages` for steering the next turn with synthetic input. Panic safety follows the same pattern as event subscribers (Agent::dispatch_event) — never let a callback crash the agent.

**Alternatives rejected**:
- **Sync hooks (observe-only)**: Too limiting — hooks that persist to disk or call APIs need async. Observe-only hooks can still return `Continue`.
- **Separate hook types for control vs observation**: Adds two traits where one suffices. A hook that only observes simply returns `Continue`.
- **Hooks that can modify the assistant message**: Would violate the principle that the message log is append-only and inspectable. Hooks influence the *next* turn, not the current one.

### D3: BudgetGuard as pre-call check (vs LoopPolicy post-turn)

**Decision**: `BudgetGuard` is a simple struct with `max_cost: Option<f64>` and `max_tokens: Option<u64>`. Its `check(&self, usage, cost) -> Result<(), BudgetExceeded>` method is `const fn` and is called before each LLM call in the inner loop. It operates at a different phase than `LoopPolicy` (pre-call vs post-turn).

**Rationale**: Loop policies run after a turn completes — by then, the LLM call has already consumed tokens and cost. BudgetGuard prevents the call from starting if the budget is already exhausted, providing tighter spend control. The `const fn` check has zero runtime overhead. The two mechanisms are complementary and independent: BudgetGuard is a hard gate, LoopPolicy is a soft post-turn decision.

**Alternatives rejected**:
- **Merge budget guard into LoopPolicy**: Would lose the pre-call timing. A policy can only stop the loop after a turn, not prevent the next LLM call from starting.
- **Budget guard as stream middleware**: Would add complexity to the streaming path. The guard is a simple check before the call, not a stream transformation.
- **Budget guard with cancellation during streaming**: The spec mentions mid-stream cancellation via CancellationToken, but the implementation uses pre-call gating for simplicity. Mid-stream cancellation can be added later if needed.

### D4: StreamMiddleware as decorator

**Decision**: `StreamMiddleware` wraps an `Arc<dyn StreamFn>` and implements `StreamFn` itself. It holds a `MapStreamFn` closure that transforms the inner stream. Convenience constructors provide common patterns: `with_logging` (inspect), `with_map` (transform each event), `with_filter` (drop events). Middleware composes by wrapping — each layer is an `Arc<dyn StreamFn>` that can be wrapped by another `StreamMiddleware`.

**Rationale**: The decorator pattern is the natural fit for stream interception — it mirrors the `ToolMiddleware` pattern already established in the codebase. Wrapping `Arc<dyn StreamFn>` means middleware composes without knowing about each other. The convenience constructors cover 90% of use cases without requiring users to deal with pinned boxed streams directly.

**Alternatives rejected**:
- **Event callback list (observer pattern)**: Would not support filtering or transformation — only inspection. The decorator pattern supports all three.
- **StreamFn wrapper trait (separate from StreamFn)**: Would require callers to handle two different stream types. By implementing StreamFn, middleware is transparent to consumers.
- **Macro-based middleware composition**: Over-engineered. Function composition via wrapping is simple and idiomatic.

### D5: In-memory MetricsCollector as async trait

**Decision**: `MetricsCollector` is an async trait with `on_metrics(&self, metrics: &TurnMetrics) -> Pin<Box<dyn Future<Output = ()> + Send>>`. `TurnMetrics` captures turn index, LLM call duration, per-tool execution metrics (name, duration, success), token usage, cost, and total turn duration. Both `TurnMetrics` and `ToolExecMetrics` derive `Serialize`/`Deserialize` for persistence.

**Rationale**: Async because collectors may flush to external monitoring systems (Prometheus, DataDog, etc.). The trait takes `&TurnMetrics` by reference so the loop retains ownership — collectors can clone what they need. Serde derives enable JSON export for dashboards. The trait has a single method because per-turn is the natural aggregation boundary — collectors that want session-level summaries can accumulate internally.

**Alternatives rejected**:
- **Sync collector**: Would block the loop during I/O. Async allows non-blocking flush.
- **Pull-based metrics (query after run)**: Would require the loop to accumulate all metrics internally. Push-based (trait callback) keeps the loop lean and lets collectors decide what to retain.
- **Tracing-based metrics (spans/events)**: Would couple metrics to the tracing ecosystem. A trait is more flexible — tracing integration is one possible implementation, not the only one.

### D6: CheckpointStore as async trait with io::Result

**Decision**: `CheckpointStore` is an async trait with four methods: `save_checkpoint`, `load_checkpoint`, `list_checkpoints`, `delete_checkpoint`. All return `Pin<Box<dyn Future<Output = io::Result<T>> + Send>>`. `Checkpoint` captures conversation-level state (messages, system prompt, model, turn count, usage, cost, metadata). `LoopCheckpoint` captures loop-level state (pending messages, overflow signal, last assistant message) and can convert to a standard `Checkpoint` via `to_checkpoint()`.

**Rationale**: `io::Result` is the natural error type for storage operations. The async trait allows implementations backed by filesystem, database, or cloud storage. Two checkpoint types serve different use cases: `Checkpoint` is the stable, portable format for conversation persistence; `LoopCheckpoint` is the internal format for pause/resume with loop-specific state (overflow signal, pending messages). The `to_checkpoint()` conversion enables storing loop state via any `CheckpointStore`. Custom messages are filtered out during checkpoint creation because they are not serializable.

**Alternatives rejected**:
- **Single checkpoint type**: Would either bloat the portable format with loop internals or lose loop state for pause/resume. Two types serve both needs cleanly.
- **Custom error type**: `io::Result` is sufficient and well-understood. Storage errors are inherently I/O errors.
- **Automatic checkpointing in the loop**: Would couple the loop to a specific checkpointing strategy. Opt-in via `PostTurnHook` or explicit calls keeps the loop lean.
