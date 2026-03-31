# Feature Specification: Tool System Extensions

**Feature Branch**: `007-tool-system-extensions`
**Created**: 2026-03-20
**Status**: Draft
**Input**: Extended tool system capabilities beyond the base trait: tool call transformers, validators, middleware, execution policies, closure-based tools, built-in tools (shell, file read, file write) behind a feature gate, auto-schema generation via proc macro, tool hot-reloading, tool filtering with patterns, noop tool for history compatibility, and tool confirmation payloads. References: PRD §4 (Tool System), HLD Implementations Layer (transformer, tool_mw, sub_agent), HLD Tool System architecture doc.

## Supersession Notice

> **Partially superseded by [031-policy-slots](../031-policy-slots/spec.md).**
>
> The following concepts from this spec are replaced by the configurable policy slot system in 031:
> - **ToolCallTransformer** (US1) → replaced by `PreDispatchPolicy` slot (Slot 2). Argument mutation is supported via `&mut arguments` in `ToolPolicyContext`.
> - **ToolValidator** (US2) → replaced by `PreDispatchPolicy` slot (Slot 2). Rejection is expressed as `PolicyVerdict::Skip(error_text)`.
> - **Tool dispatch pipeline order** (FR-004: approval → transformer → validator → schema → execute) → new order is: **PreDispatch policies → Approval gate → Schema validation (hardcoded) → Execute**. See 031 FR-007, FR-008.
>
> The following concepts from this spec **remain valid and are NOT affected by 031**:
> - **ToolMiddleware** (US3) — wraps `execute()`, not the dispatch pipeline. Not a policy.
> - **ToolExecutionPolicy** (US4) — controls concurrency, not policy decisions. Stays as-is.
> - **FnTool / closure-based tools** (US5) — unchanged.
> - **Built-in tools** (US6) — unchanged.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Rewrite Tool Calls Before Execution (Priority: P1)

A developer registers a tool call transformer that rewrites tool calls before they are executed. The transformer can modify arguments or inject additional parameters. It runs unconditionally on every tool call — before validation and before execution — enabling cross-cutting argument manipulation. Note: the transformer receives the tool name as a read-only reference (`&str`) and cannot rename tools.

**Why this priority**: Transformers enable critical use cases like parameter injection (adding session IDs), argument sanitization, and tool aliasing without modifying individual tool implementations.

**Independent Test**: Can be tested by registering a transformer that modifies an argument, invoking a tool, and verifying the tool receives the modified arguments.

**Acceptance Scenarios**:

1. **Given** a registered transformer, **When** a tool call is made, **Then** the transformer runs before validation and execute.
2. **Given** a transformer that modifies arguments, **When** the tool executes, **Then** it receives the modified arguments.
3. **Given** no transformer registered, **When** a tool call is made, **Then** the original arguments pass through unchanged.

---

### User Story 2 - Validate Tool Calls Before Execution (Priority: P1)

A developer registers a tool validator that inspects tool calls after transformation but before execution. The validator can reject a call with an error result, preventing execute from being invoked. This is distinct from the transformer (which rewrites) — the validator only accepts or rejects.

**Why this priority**: Validation gates enable security controls, rate limiting, and policy enforcement without modifying tool implementations.

**Independent Test**: Can be tested by registering a validator that rejects a specific tool name and verifying the tool is not executed.

**Acceptance Scenarios**:

1. **Given** a registered validator, **When** a tool call passes validation, **Then** the tool executes normally.
2. **Given** a registered validator, **When** a tool call fails validation, **Then** an error result is returned without calling execute.
3. **Given** the dispatch pipeline, **When** a tool call is processed, **Then** the order is: transformer → validator → schema validation → execute.

---

### User Story 3 - Wrap Tool Execution with Middleware (Priority: P2)

A developer wraps a tool's execute function with middleware that adds cross-cutting behavior: logging, metrics, access control, or caching. Middleware follows the decorator pattern — it wraps the inner tool without modifying it, and multiple middleware can be composed.

**Why this priority**: Middleware enables observability and governance without coupling those concerns to individual tool implementations.

**Independent Test**: Can be tested by wrapping a tool with logging middleware and verifying the middleware runs around the tool's execute call.

**Acceptance Scenarios**:

1. **Given** middleware wrapping a tool, **When** the tool is executed, **Then** the middleware runs before and/or after the inner execute.
2. **Given** multiple middleware composed, **When** the tool is executed, **Then** middleware runs in the configured order.
3. **Given** middleware, **When** it wraps a tool, **Then** the tool's name, description, and schema remain unchanged.

---

### User Story 4 - Control Tool Execution Order (Priority: P2)

A developer configures the tool execution policy for a batch of tool calls within a single turn. The default is concurrent execution, but the developer can choose sequential execution or priority-based ordering for tools that have dependencies or ordering requirements.

**Why this priority**: Most use cases work with concurrent execution, but some tool combinations (e.g., write-then-read) need ordering control.

**Independent Test**: Can be tested by configuring sequential policy and verifying tools execute one after another rather than concurrently.

**Acceptance Scenarios**:

1. **Given** concurrent execution policy (default), **When** multiple tool calls are dispatched, **Then** they run concurrently.
2. **Given** sequential execution policy, **When** multiple tool calls are dispatched, **Then** they run one after another in order.
3. **Given** priority execution policy, **When** multiple tool calls are dispatched, **Then** they run in priority order.

---

### User Story 5 - Create Tools from Closures (Priority: P2)

A developer creates a simple tool from a closure without defining a full tool struct. They provide a name, description, parameter schema, and an async closure — the system wraps it into a tool implementation automatically. This is a convenience for tools that don't need complex state.

**Why this priority**: Reduces boilerplate for simple tools, but the full trait is needed for complex tools with state.

**Independent Test**: Can be tested by creating a closure-based tool, registering it, and verifying it executes correctly when called.

**Acceptance Scenarios**:

1. **Given** a name, description, schema, and closure, **When** a closure-based tool is created, **Then** it implements the tool trait.
2. **Given** a closure-based tool, **When** it is invoked by the agent, **Then** the closure receives the arguments and returns a result.

---

### User Story 6 - Use Built-In Shell and File Tools (Priority: P3)

A developer enables the built-in tools feature to get pre-made tools for shell command execution, file reading, and file writing. These tools are gated behind a feature flag so they can be excluded from builds that don't need them.

**Why this priority**: Built-in tools are a convenience — many agents need shell/file access, but the tools are optional and behind a feature gate.

**Independent Test**: Can be tested by enabling the feature flag, registering built-in tools, and verifying they execute correctly.

**Acceptance Scenarios**:

1. **Given** the built-in tools feature enabled, **When** the developer registers built-in tools, **Then** shell, file read, and file write tools are available.
2. **Given** the built-in tools feature disabled, **When** the developer builds the crate, **Then** built-in tools are not available and the crate compiles without them.
3. **Given** a built-in tool, **When** it is invoked, **Then** it respects the cancellation token for cooperative cancellation.

---

### User Story 7 - Auto-Schema Generation from Rust Types (Priority: P1) — C12

A developer uses a `#[derive(ToolSchema)]` proc macro to generate JSON Schema from a Rust struct definition. Field names map to properties, Rust types map to JSON Schema types (`String` → `"string"`, `u64` → `"integer"`, `bool` → `"boolean"`, `Option<T>` → nullable, `Vec<T>` → `"array"`), and `///` doc comments become `description` fields. A `#[tool]` attribute macro wraps an async function as an `AgentTool` implementation, generating the struct, schema, and trait impl from the function signature.

**Why this priority**: Schema generation from type definitions eliminates the most common source of tool definition bugs — hand-written JSON Schema that drifts from the actual parameter types. Every tool author benefits.

**Independent Test**: Can be tested by defining a struct with `#[derive(ToolSchema)]`, calling `ToolParameters::json_schema()`, and verifying the output matches the expected JSON Schema.

**Acceptance Scenarios**:

1. **Given** a struct with `#[derive(ToolSchema)]`, **When** `json_schema()` is called, **Then** a valid JSON Schema is returned with correct type mappings and descriptions from doc comments.
2. **Given** an async function with `#[tool(name = "...", description = "...")]`, **When** the macro is expanded, **Then** a struct implementing `AgentTool` is generated with the correct schema derived from function parameters.
3. **Given** a field with `#[tool(description = "...")]` attribute, **When** the schema is generated, **Then** the attribute description overrides the doc comment.
4. **Given** an `Option<T>` field, **When** the schema is generated, **Then** the field is not in `required` and its type is nullable.

---

### User Story 8 - Tool Hot-Reloading (Priority: P2) — I12

An operator configures a directory containing tool definitions (TOML/YAML/JSON files specifying a command to execute). A `ToolWatcher` monitors this directory for changes and reloads tool definitions at runtime, updating the agent's tool list via `Agent::set_tools()`.

**Why this priority**: Hot-reloading enables tool iteration without restarting the agent — valuable for development and for dynamically loaded MCP-style tools in production.

**Independent Test**: Can be tested by starting a watcher on a temp directory, adding a tool definition file, and verifying the agent's tool list is updated.

**Acceptance Scenarios**:

1. **Given** a `ToolWatcher` monitoring a directory, **When** a new tool definition file is added, **Then** the corresponding tool is added to the agent's tool list.
2. **Given** a watched directory, **When** a tool definition file is modified, **Then** the tool is reloaded with the updated definition.
3. **Given** a watched directory, **When** a tool definition file is deleted, **Then** the tool is removed from the agent's tool list.
4. **Given** a TOML tool definition specifying a shell command, **When** the tool is invoked, **Then** the command executes with the provided arguments.
5. **Given** the `hot-reload` feature is disabled, **When** the crate is compiled, **Then** no `notify` dependencies are included.

---

### User Story 9 - Tool Filtering with Patterns (Priority: P2) — I13

A developer configures a `ToolFilter` with `allowed` and `rejected` patterns that are applied at tool registration time. Patterns support exact string matching, glob syntax (`read_*`), and regex (`^file_.*$`). This restricts which tools are available to the agent — useful for dynamically loaded tools (MCP, hot-reload) where the full set may not be trusted.

**Why this priority**: Filtering at registration time provides a coarser-grained security boundary than per-call policies — tools that don't pass the filter never appear in the LLM prompt.

**Independent Test**: Can be tested by creating a `ToolFilter` with an allowed pattern, registering tools, and verifying only matching tools are available.

**Acceptance Scenarios**:

1. **Given** a `ToolFilter` with `allowed: ["read_*"]`, **When** tools are registered, **Then** only tools matching `read_*` are added.
2. **Given** a `ToolFilter` with `rejected: ["bash"]`, **When** tools are registered, **Then** the `bash` tool is excluded.
3. **Given** both `allowed` and `rejected` patterns, **When** a tool matches both, **Then** `rejected` takes precedence — the tool is excluded.
4. **Given** a regex pattern `^file_.*$` in `allowed`, **When** tools are registered, **Then** only tools with names matching the regex are added.
5. **Given** no `ToolFilter` configured, **When** tools are registered, **Then** all tools are accepted (backward compatible).

---

### User Story 10 - Noop Tool for Session History Compatibility (Priority: P3) — N5

When loading a session that references a tool no longer in the agent's registry, the system auto-injects a `NoopTool` placeholder. The `NoopTool` returns an error result explaining the tool is no longer available, preventing deserialization failures when tool sets evolve across sessions.

**Why this priority**: Nice-to-have for long-lived agents with persistent sessions — prevents crashes when tools are added/removed between runs.

**Independent Test**: Can be tested by loading a session with a tool call referencing a non-existent tool and verifying the agent handles it gracefully with a `NoopTool`.

**Acceptance Scenarios**:

1. **Given** a session referencing tool `"old_tool"` that is no longer registered, **When** the session is loaded, **Then** a `NoopTool` for `"old_tool"` is auto-injected.
2. **Given** a `NoopTool` for `"old_tool"`, **When** the LLM calls `"old_tool"`, **Then** it receives an error result saying the tool is no longer available.
3. **Given** the session contains tool results from `"old_tool"`, **When** the session is loaded, **Then** existing results are preserved — the `NoopTool` only handles new invocations.

---

### User Story 11 - Tool Confirmation Payloads (Priority: P3) — N6

A tool provides rich context to the approval UI by implementing an optional `approval_context` method. When a tool call requires approval, the system calls `approval_context(&self, params: &Value) -> Option<Value>` and attaches the result to the `ToolApprovalRequest`. The approval UI can display this context (e.g., a diff preview for file writes, estimated cost for API calls, a query plan for database tools).

**Why this priority**: Nice-to-have that enriches the approval experience — tools work fine without it, but approval decisions are better informed when context is available.

**Independent Test**: Can be tested by implementing `approval_context` on a tool, triggering an approval request, and verifying the context is attached to the `ToolApprovalRequest`.

**Acceptance Scenarios**:

1. **Given** a tool implementing `approval_context` that returns `Some(context)`, **When** the tool call requires approval, **Then** the `ToolApprovalRequest` includes the context in its `context` field.
2. **Given** a tool that does not override `approval_context` (default `None`), **When** the tool call requires approval, **Then** the `ToolApprovalRequest.context` is `None`.
3. **Given** a `ToolApprovalRequest` with context, **When** the approval UI renders it, **Then** the context is available for display.

---

### Edge Cases

- What happens when a transformer rewrites a tool name to one that doesn't exist — the transformer only modifies arguments, not the tool name. Unknown tools produce an error result via `unknown_tool_result()`.
- What happens when middleware modifies the tool result — yes, the loop sees the modified result since middleware wraps `execute()`.
- How does the system handle a closure-based tool that panics — panics in spawned tool tasks are caught via join error handling and converted to error results.
- What happens when the execution policy is sequential but a steering interrupt arrives — remaining sequential tools are skipped; the `steering_detected` atomic flag causes cancellation between groups.
- What happens when the `#[tool]` macro is applied to a non-async function — compile error. The macro requires `async fn`.
- What happens when a hot-reloaded tool definition has an invalid schema — the tool is rejected with a logged warning; existing tools are unaffected.
- How does ScriptTool handle parameter interpolation security — all parameter values are shell-escaped before interpolation into the command template to prevent command injection via LLM-controlled parameters.
- What happens when two definition files define tools with the same name — last-write-wins; the most recently modified file's definition takes precedence. A warning is logged when a duplicate is detected.
- What happens when a `ToolFilter` is applied after tools are already registered — the filter is applied to the current tool list, removing non-matching tools.
- What happens when a `NoopTool` receives arguments — it ignores them and returns the standard "tool no longer available" error message.
- What happens when `approval_context` panics — the panic is caught via `catch_unwind`, logged, and `context` is set to `None`. The approval request proceeds without context.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST support a tool call transformer that rewrites tool calls before validation and execution. It MUST run unconditionally on every tool call.
- **FR-002**: The transformer MUST be distinct from the validator — it rewrites, not rejects.
- **FR-003**: System MUST support a tool validator that accepts or rejects tool calls after transformation but before execution. Rejected calls MUST produce an error result without invoking execute.
- **FR-004**: **[Superseded by 031]** The tool dispatch pipeline order is now: PreDispatch policies (Slot 2) → approval → schema validation (hardcoded) → execute. See 031 FR-007, FR-008.
- **FR-005**: System MUST support tool middleware that wraps the execute function using the decorator pattern, enabling composable cross-cutting behavior.
- **FR-006**: Middleware MUST NOT alter the tool's name, description, or schema — only the execution behavior.
- **FR-007**: System MUST support configurable tool execution policies: concurrent (default), sequential, and priority.
- **FR-008**: System MUST provide a convenience mechanism for creating tools from closures without defining a full struct.
- **FR-009**: System MUST provide built-in tools for shell execution, file reading, and file writing, gated behind an optional feature flag that is enabled by default.
- **FR-010**: Built-in tools MUST respect the cancellation token for cooperative cancellation.
- **FR-011**: Built-in tools MUST define appropriate parameter schemas for argument validation.
- **FR-012**: System MUST provide a `#[derive(ToolSchema)]` proc macro in a separate `swink-agent-macros` crate that generates JSON Schema from Rust struct definitions, mapping field types and doc comments to schema properties and descriptions.
- **FR-013**: System MUST provide a `#[tool]` attribute macro that wraps an async function as an `AgentTool` implementation, generating the struct, schema, and trait impl from the function signature.
- **FR-014**: The proc macro crate MUST be optional — tools can still be defined manually via `AgentTool` trait or `FnTool` builder.
- **FR-015**: System MUST provide a feature-gated (`hot-reload`) `ToolWatcher` that monitors a directory for tool definition files (TOML/YAML/JSON) and reloads them at runtime.
- **FR-016**: Tool definition files MUST specify at minimum: `name`, `description`, `command` (shell command to execute), and optionally `parameters_schema`.
- **FR-017**: System MUST provide a `ToolFilter` struct with `allowed` and `rejected` pattern lists supporting exact string, glob, and regex matching, applied at tool registration time.
- **FR-018**: When both `allowed` and `rejected` patterns match a tool name, `rejected` MUST take precedence.
- **FR-019**: System MUST auto-inject `NoopTool` placeholders when loading sessions that reference tools no longer in the registry, returning an error result explaining the tool is unavailable.
- **FR-020**: The `AgentTool` trait MUST include a default method `approval_context(&self, params: &Value) -> Option<Value>` returning `None`, allowing tools to provide rich context for the approval UI.
- **FR-021**: `ToolApprovalRequest` MUST include a `context: Option<Value>` field populated from `approval_context()` when the tool requires approval.

### Key Entities

- **ToolCallTransformer**: **[Superseded by 031]** Replaced by `PreDispatchPolicy` slot (Slot 2) with `&mut arguments` support.
- **ToolValidator**: **[Superseded by 031]** Replaced by `PreDispatchPolicy` slot (Slot 2) with `PolicyVerdict::Skip` for rejection.
- **ToolMiddleware**: Decorator wrapping a tool's execute function — composable cross-cutting behavior.
- **ToolExecutionPolicy**: Configuration controlling how a batch of tool calls is executed: concurrent, sequential, or priority.
- **FnTool**: Convenience wrapper that creates a tool from a closure.
- **BashTool**: Built-in shell command execution tool (feature-gated).
- **ReadFileTool**: Built-in file reading tool (feature-gated).
- **WriteFileTool**: Built-in file writing tool (feature-gated).
- **ToolSchema (derive macro)**: Proc macro generating JSON Schema from Rust struct definitions. Lives in `swink-agent-macros` crate.
- **#[tool] (attribute macro)**: Wraps an async function as an `AgentTool` implementation. Lives in `swink-agent-macros` crate.
- **ToolParameters (trait)**: Trait with `fn json_schema() -> Value` implemented by `#[derive(ToolSchema)]`.
- **ToolWatcher**: Feature-gated (`hot-reload`) directory watcher that monitors for tool definition file changes and reloads tools at runtime.
- **ScriptTool**: A tool loaded from a definition file (TOML/YAML/JSON) that executes a shell command.
- **ToolFilter**: Pattern-based tool filtering at registration time. Supports exact, glob, and regex patterns.
- **NoopTool**: Placeholder tool auto-injected for session history compatibility when a referenced tool no longer exists.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Tool call transformers correctly rewrite arguments before they reach validation or execution.
- **SC-002**: Tool validators correctly reject invalid calls without invoking execute.
- **SC-003**: The dispatch pipeline enforces the correct order: approval → transformer → validator → schema → execute.
- **SC-004**: Tool middleware wraps execution without altering the tool's identity (name, description, schema).
- **SC-005**: Execution policies correctly control concurrency: concurrent runs in parallel, sequential runs in order.
- **SC-006**: Closure-based tools implement the tool trait and execute correctly when invoked.
- **SC-007**: Built-in tools are available when the feature flag is enabled and absent when disabled.
- **SC-008**: `#[derive(ToolSchema)]` generates correct JSON Schema from Rust types, mapping `String` → `"string"`, `u64` → `"integer"`, `Option<T>` → not required, `Vec<T>` → `"array"`, and doc comments → `description`.
- **SC-009**: `#[tool]` attribute macro generates a valid `AgentTool` implementation from an async function signature.
- **SC-010**: `ToolWatcher` detects file additions, modifications, and deletions in the watched directory and updates the agent's tool list accordingly.
- **SC-011**: `ToolFilter` correctly applies exact, glob, and regex patterns, with `rejected` taking precedence over `allowed`.
- **SC-012**: `NoopTool` placeholders are auto-injected for missing tools during session loading, returning descriptive error results.
- **SC-013**: `approval_context()` values are correctly attached to `ToolApprovalRequest` and available to the approval UI.

## Clarifications

### Session 2026-03-20

- Q: Can transformers rewrite tool names to nonexistent tools? → A: Transformers only modify arguments, not tool names. Unknown tools get error results.
- Q: Does middleware-modified result reach the loop? → A: Yes, middleware wraps execute(), so the loop sees modified results.
- Q: Are closure tool panics caught? → A: Yes, panics in spawned tasks are caught and converted to error results.
- Q: Does steering skip remaining sequential tools? → A: Yes, the steering_detected flag causes remaining groups to be cancelled.

### Session 2026-03-31

- Q: Should ScriptTool shell-escape interpolated parameter values to prevent command injection? → A: Always shell-escape parameter values before interpolation. Command injection via LLM-controlled parameters is a real attack surface. Escaping is the safe default.
- Q: How should ToolWatcher handle duplicate tool names across definition files? → A: Last-write-wins — the most recently modified file's definition takes precedence. A warning is logged when a duplicate is detected.
- Q: Should `hot-reload-dylib` (dynamic library loading via `libloading`) be in scope? → A: Out of scope for this iteration. Dynamic library loading requires `unsafe` code (violates `#[forbid(unsafe_code)]`) and adds significant complexity. Script-based tools cover the practical hot-reload use case. Revisit in a future spec if demand materializes.
- Q: What debounce interval should ToolWatcher use for filesystem events? → A: 500ms — standard debounce for file watchers. Long enough to coalesce editor save events, short enough to feel responsive during development.

## Assumptions

- **[Superseded by 031]** Tool call transformers and validators are replaced by PreDispatch policies (Slot 2). Policies run unconditionally before approval, matching the original transformer behavior.
- **[Superseded by 031]** The dispatch pipeline order is now: PreDispatch policies → approval → schema validation → execute. See 031.
- Built-in tools are enabled by default via the `builtin-tools` feature flag on the core crate.
- Closure-based tools support async execution and cancellation tokens.
- Middleware composition order is determined by the order middleware is applied (outermost wraps first).
- The `swink-agent-macros` proc macro crate is a new workspace member. It depends only on `syn`, `quote`, and `proc-macro2` — no runtime dependency on `swink-agent`.
- Hot-reloading uses the `notify` crate for filesystem watching, feature-gated behind `hot-reload`.
- Dynamic library loading (`libloading`) is out of scope for this iteration — it requires `unsafe` code and is deferred to a future spec.
- Tool filtering is always available (no feature gate) since it has no external dependencies.
- `NoopTool` is always available — it is a zero-dependency struct in the core crate.
- `approval_context()` is a default method on `AgentTool` returning `None` — backward compatible, no existing tool implementations need changes.
