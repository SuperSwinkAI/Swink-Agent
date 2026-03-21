# Feature Specification: Tool System Extensions

**Feature Branch**: `007-tool-system-extensions`
**Created**: 2026-03-20
**Status**: Draft
**Input**: Extended tool system capabilities beyond the base trait: tool call transformers, validators, middleware, execution policies, closure-based tools, and built-in tools (shell, file read, file write) behind a feature gate. References: PRD §4 (Tool System), HLD Implementations Layer (transformer, tool_mw, sub_agent), HLD Tool System architecture doc.

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

### Edge Cases

- What happens when a transformer rewrites a tool name to one that doesn't exist — the transformer only modifies arguments, not the tool name. Unknown tools produce an error result via `unknown_tool_result()`.
- What happens when middleware modifies the tool result — yes, the loop sees the modified result since middleware wraps `execute()`.
- How does the system handle a closure-based tool that panics — panics in spawned tool tasks are caught via join error handling and converted to error results.
- What happens when the execution policy is sequential but a steering interrupt arrives — remaining sequential tools are skipped; the `steering_detected` atomic flag causes cancellation between groups.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST support a tool call transformer that rewrites tool calls before validation and execution. It MUST run unconditionally on every tool call.
- **FR-002**: The transformer MUST be distinct from the validator — it rewrites, not rejects.
- **FR-003**: System MUST support a tool validator that accepts or rejects tool calls after transformation but before execution. Rejected calls MUST produce an error result without invoking execute.
- **FR-004**: The tool dispatch pipeline order MUST be: approval → transformer → validator → schema validation → execute.
- **FR-005**: System MUST support tool middleware that wraps the execute function using the decorator pattern, enabling composable cross-cutting behavior.
- **FR-006**: Middleware MUST NOT alter the tool's name, description, or schema — only the execution behavior.
- **FR-007**: System MUST support configurable tool execution policies: concurrent (default), sequential, and priority.
- **FR-008**: System MUST provide a convenience mechanism for creating tools from closures without defining a full struct.
- **FR-009**: System MUST provide built-in tools for shell execution, file reading, and file writing, gated behind an optional feature flag that is enabled by default.
- **FR-010**: Built-in tools MUST respect the cancellation token for cooperative cancellation.
- **FR-011**: Built-in tools MUST define appropriate parameter schemas for argument validation.

### Key Entities

- **ToolCallTransformer**: Hook that rewrites tool calls before validation — modifies arguments, renames tools, injects parameters.
- **ToolValidator**: Hook that accepts or rejects tool calls after transformation — distinct from transformer (rejects vs rewrites).
- **ToolMiddleware**: Decorator wrapping a tool's execute function — composable cross-cutting behavior.
- **ToolExecutionPolicy**: Configuration controlling how a batch of tool calls is executed: concurrent, sequential, or priority.
- **FnTool**: Convenience wrapper that creates a tool from a closure.
- **BashTool**: Built-in shell command execution tool (feature-gated).
- **ReadFileTool**: Built-in file reading tool (feature-gated).
- **WriteFileTool**: Built-in file writing tool (feature-gated).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Tool call transformers correctly rewrite arguments before they reach validation or execution.
- **SC-002**: Tool validators correctly reject invalid calls without invoking execute.
- **SC-003**: The dispatch pipeline enforces the correct order: approval → transformer → validator → schema → execute.
- **SC-004**: Tool middleware wraps execution without altering the tool's identity (name, description, schema).
- **SC-005**: Execution policies correctly control concurrency: concurrent runs in parallel, sequential runs in order.
- **SC-006**: Closure-based tools implement the tool trait and execute correctly when invoked.
- **SC-007**: Built-in tools are available when the feature flag is enabled and absent when disabled.

## Clarifications

### Session 2026-03-20

- Q: Can transformers rewrite tool names to nonexistent tools? → A: Transformers only modify arguments, not tool names. Unknown tools get error results.
- Q: Does middleware-modified result reach the loop? → A: Yes, middleware wraps execute(), so the loop sees modified results.
- Q: Are closure tool panics caught? → A: Yes, panics in spawned tasks are caught and converted to error results.
- Q: Does steering skip remaining sequential tools? → A: Yes, the steering_detected flag causes remaining groups to be cancelled.

## Assumptions

- The tool call transformer runs unconditionally — it is not gated by approval. This is distinct from the validator.
- The dispatch pipeline order is fixed and not configurable by the caller.
- Built-in tools are enabled by default via the `builtin-tools` feature flag on the core crate.
- Closure-based tools support async execution and cancellation tokens.
- Middleware composition order is determined by the order middleware is applied (outermost wraps first).
