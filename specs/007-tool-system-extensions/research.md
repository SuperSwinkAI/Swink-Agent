# Research: Tool System Extensions

**Feature**: 007-tool-system-extensions | **Date**: 2026-03-20

## Design Decisions

### D1: ToolCallTransformer as a Trait with Blanket Closure Impl

**Decision**: Define `ToolCallTransformer` as a trait with a single `transform(&self, tool_name: &str, arguments: &mut Value)` method, plus a blanket `impl` for closures matching `Fn(&str, &mut Value)`.

**Rationale**: The trait approach allows both struct-based implementations (with state, configuration, or multiple trait impls) and zero-boilerplate closures via the blanket impl. Mutation in place (`&mut Value`) avoids cloning arguments on every tool call. The transformer is synchronous because argument rewriting should not involve I/O — keeping it sync simplifies the dispatch pipeline and avoids unnecessary async overhead.

**Alternatives rejected**:
- **Async transformer**: Would require boxing futures for a synchronous operation. No use case requires I/O during argument rewriting.
- **Returning `Option<Value>`** instead of mutation: Requires cloning the entire arguments value even when no change is needed.
- **Closure-only (no trait)**: Prevents struct-based implementations that carry configuration state.

---

### D2: ToolValidator as a Separate Trait from Transformer

**Decision**: Define `ToolValidator` as a distinct trait with `validate(&self, tool_name: &str, arguments: &Value) -> Result<(), String>`, separate from `ToolCallTransformer`.

**Rationale**: Transformers rewrite; validators accept or reject. These are fundamentally different operations with different return types. Combining them into a single hook would conflate mutation with gating, making each harder to reason about and test independently. The validator takes `&Value` (immutable) to enforce the contract that it cannot modify arguments — only inspect them.

**Alternatives rejected**:
- **Combined transformer+validator**: Conflates two distinct concerns. A transformer that also validates would need a tri-state return (pass, reject, modify), increasing complexity.
- **Async validator**: Same reasoning as transformer — validation should not require I/O.

---

### D3: ToolMiddleware as a Decorator Around execute()

**Decision**: `ToolMiddleware` wraps an `Arc<dyn AgentTool>` and intercepts `execute()` while delegating all metadata methods (`name`, `label`, `description`, `parameters_schema`, `requires_approval`) to the inner tool.

**Rationale**: The decorator pattern enables composable cross-cutting concerns (logging, timeouts, metrics, caching) without modifying tool implementations. Using `Arc<dyn AgentTool>` for the inner tool allows middleware to be composed — wrapping middleware around middleware. The middleware closure receives the inner tool as an `Arc`, enabling it to call through to the original `execute()` at any point.

**Alternatives rejected**:
- **Trait-based middleware chain**: More complex to implement and compose. The closure-based approach is simpler and covers all use cases.
- **Event-based hooks**: Would require the event system to support mutation of execution flow, violating the "events are outward-only" constraint.

---

### D4: ToolExecutionPolicy as an Enum with Custom Strategy Escape Hatch

**Decision**: Define `ToolExecutionPolicy` as an enum with four variants: `Concurrent` (default), `Sequential`, `Priority(Arc<PriorityFn>)`, and `Custom(Arc<dyn ToolExecutionStrategy>)`.

**Rationale**: The three named variants cover the vast majority of use cases without any callback overhead. The `Custom` variant provides an escape hatch for advanced scenarios via the `ToolExecutionStrategy` trait. Priority groups execute concurrently within a priority level and sequentially across levels — this is the natural model for tools with partial ordering constraints.

**Alternatives rejected**:
- **Trait-only (no enum)**: Forces callers to implement a trait even for the common case of "just run everything concurrently."
- **Configuration struct with boolean flags**: Does not naturally extend to priority-based or custom ordering.
- **Hardcoded concurrent-only**: Insufficient for tools with write-then-read dependencies.

---

### D5: FnTool as a Builder with Closure Storage

**Decision**: `FnTool` stores name, label, description, schema, approval flag, and an `Arc<ExecuteFn>` closure. Builder methods (`with_schema_for`, `with_execute_simple`, `with_execute`) configure each field. It implements `AgentTool`.

**Rationale**: Many tools are simple functions that do not need struct state. `FnTool` eliminates the boilerplate of defining a struct and implementing five trait methods. The `with_execute_simple` variant takes only `(Value, CancellationToken)` for the common case where the tool call ID and update callback are unused. `Arc` wrapping the closure enables `Clone` if needed.

**Alternatives rejected**:
- **Macro-based tool generation**: Harder to debug, less discoverable, and does not compose with middleware.
- **Free function registration**: Cannot carry per-tool configuration (schema, approval requirements).

---

### D6: Built-in Tools Behind a Default-Enabled Feature Gate

**Decision**: `BashTool`, `ReadFileTool`, and `WriteFileTool` live in `src/tools/` behind the `builtin-tools` feature flag, which is enabled by default. A `builtin_tools()` convenience function returns all three wrapped in `Arc`.

**Rationale**: Most agents need shell and file access, so enabling by default reduces friction. The feature gate allows library consumers who do not need built-in tools (e.g., pure-API agents) to exclude them, avoiding the `tokio::process` dependency on platforms that do not support it. Each tool follows the same pattern: schema as a `Value` field, `schemars` derivation for params, cancellation pre-check before I/O.

**Alternatives rejected**:
- **Always included (no feature gate)**: Forces all consumers to compile shell execution code even when unused.
- **Separate crate**: The tools are small and tightly coupled to `AgentTool`. A separate crate adds workspace complexity without meaningful boundary benefit.

---

### D7: Fixed Dispatch Pipeline Order

**Decision**: The tool dispatch pipeline order is fixed and not configurable: approval, transformer, validator, schema validation, execute.

**Rationale**: A fixed order eliminates configuration complexity and makes the system predictable. The order is logical: approval gates whether the call should proceed at all, transformation rewrites arguments, validation rejects invalid arguments, schema validation enforces structural correctness, and execute runs the tool. Reordering these steps would create confusing semantics (e.g., validating before transforming would reject arguments that the transformer is about to fix).

**Alternatives rejected**:
- **Configurable pipeline ordering**: Adds complexity with no clear use case. Every reasonable ordering is equivalent to the fixed one.
- **Multiple transformer/validator stages**: Over-engineering. Composing multiple transformers can be done within a single transformer implementation.
