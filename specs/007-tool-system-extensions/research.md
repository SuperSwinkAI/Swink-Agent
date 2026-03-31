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

---

### D8: Auto-Schema via Proc Macro in Separate Crate

**Decision**: Create a `swink-agent-macros` crate containing `#[derive(ToolSchema)]` and `#[tool]` proc macros. The `ToolParameters` trait (with `fn json_schema() -> Value`) is defined in the core crate. The macro crate is an optional dependency — tools can still use `FnTool::with_schema_for::<T>()` or manual JSON Schema.

**Rationale**: Proc macros must live in a separate crate (Rust constraint). Defining the trait in core and the derive in the macro crate follows the standard pattern (`serde` / `serde_derive`). Making the macro crate optional preserves the existing `FnTool` and manual `AgentTool` impl paths — no existing code needs changes. The `#[tool]` attribute macro is strictly additive, converting a common 30-line boilerplate pattern into a single annotation.

**Key references**: AWS Strands' `@tool` decorator auto-generates schema from Python type hints + docstrings. Google ADK's `build_function_declaration()` extracts from Python function signatures. In Rust, `schemars` already does type → JSON Schema; our macro wraps this with tool-specific ergonomics.

**Alternatives rejected**:
- **Extend `schemars` derive directly**: Does not generate tool-specific schema structure (name, description from doc comments, required/optional from `Option<T>`). A custom derive provides the right abstraction level.
- **Runtime reflection**: Not available in Rust. Proc macros are the idiomatic approach.
- **Put macro in core crate**: Proc macro crates cannot export non-macro items. The trait must be in core, the derive in a separate crate.

---

### D9: Tool Hot-Reloading via notify + TOML Definitions

**Decision**: Feature-gate hot-reloading behind `hot-reload`. Use the `notify` crate for filesystem watching. Tool definitions are TOML/YAML/JSON files specifying `name`, `description`, `command`, and optional `parameters_schema`. Loaded tools become `ScriptTool` instances that execute shell commands. Dynamic library loading (`libloading`) is a separate sub-feature (`hot-reload-dylib`).

**Rationale**: Rust can't reload compiled code without `libloading` (which requires `unsafe`). Script-based tools (shell commands from definition files) are the natural hot-reloadable unit — they match MCP-style tool definitions. `notify` is the standard Rust filesystem watcher (3M+ downloads). The two-tier feature gate (`hot-reload` for scripts, `hot-reload-dylib` for shared libraries) keeps the common case simple while allowing the advanced case.

**Alternatives rejected**:
- **WASM-based tool loading**: Over-engineered. Shell commands cover the hot-reload use case without a WASM runtime.
- **Always-on file watching**: Pulls in `notify` for all users. Feature gating respects the zero-cost principle.
- **Compiling tools from source at runtime**: Not feasible without shipping a Rust compiler. Shell commands are the practical alternative.

---

### D10: ToolFilter with Pattern Matching at Registration Time

**Decision**: `ToolFilter` applies at tool registration time (when tools are added to the agent), not at execution time. It uses `ToolPattern` (Exact/Glob/Regex) with auto-detection via `parse()`. `rejected` patterns take precedence over `allowed`.

**Rationale**: Filtering at registration time is a coarser-grained boundary than per-call policies (ToolDenyListPolicy). A tool that fails the filter never appears in the LLM prompt — the model never sees it, can't call it, and the tool's schema doesn't consume token budget. This complements `ToolDenyListPolicy` (which blocks at execution time) for defense-in-depth. The reject-wins precedence rule is the safest default — explicitly blocked tools can't be unblocked by a permissive `allowed` pattern.

**Alternatives rejected**:
- **Extend ToolDenyListPolicy**: ToolDenyListPolicy runs at execution time (per-call). Registration-time filtering serves a different purpose — removing tools from the prompt entirely.
- **Only exact matching**: Insufficient for dynamically loaded tools where names follow patterns (e.g., `mcp_*`).
- **Allowed-wins precedence**: Dangerous — an overly broad `allowed` pattern could override a specific `rejected` entry.

---

### D11: NoopTool as Auto-Injected Placeholder

**Decision**: When loading a session that references a tool not in the current registry, auto-inject a `NoopTool` with the missing tool's name. The `NoopTool` returns `AgentToolResult::error(...)` explaining the tool is no longer available.

**Rationale**: Without this, loading a session with a removed tool causes either a deserialization error or a silent `unknown_tool_result()` at dispatch time. The `NoopTool` makes the situation explicit to the model — it can adapt its strategy rather than repeatedly calling a broken tool. The tool is transparent (no special-casing in the loop) because it implements `AgentTool` like any other tool.

**Alternatives rejected**:
- **Silently drop tool calls**: The model would see no result and might retry indefinitely.
- **Error during session loading**: Too harsh — the session is otherwise valid.
- **Map to a default tool**: Unpredictable behavior. An explicit error is safer.

---

### D12: Approval Context as Optional Default Method

**Decision**: Add `fn approval_context(&self, params: &Value) -> Option<Value> { None }` as a default method on `AgentTool`. Populate `ToolApprovalRequest.context` from this method before sending to the approval callback.

**Rationale**: A default method is backward compatible — no existing tool implementations need changes. The `Option<Value>` return type allows tools to provide arbitrary structured context (diffs, cost estimates, query plans) without constraining the format. Panics in `approval_context` are caught via `catch_unwind` to prevent tool-provided context from crashing the dispatch pipeline.

**Alternatives rejected**:
- **Separate trait for contextual tools**: Adds complexity. A default method on the existing trait is simpler and more discoverable.
- **Typed context (enum of known formats)**: Over-constrains. Tools should be free to provide whatever context their approval UI needs.
- **Context as part of `execute()`**: Too late — context is needed before execution, at approval time.
