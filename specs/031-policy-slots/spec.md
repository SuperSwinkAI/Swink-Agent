# Feature Specification: Configurable Policy Slots for the Agent Loop

**Feature Branch**: `031-policy-slots`
**Created**: 2026-03-24
**Status**: Draft
**Input**: User description: "Replace scattered single-purpose hook fields on AgentLoopConfig with a unified system of four configurable policy slots at natural seam points in the agent loop"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Agent Consumer Adds Budget Enforcement (Priority: P1)

A library consumer building an agent-powered application wants to prevent runaway LLM costs. They construct an agent with a budget policy that stops the loop when accumulated cost exceeds a threshold. They add a single policy to the pre-turn slot and the loop enforces the limit automatically.

**Why this priority**: Cost control is the most common and highest-impact policy need. Without it, a misbehaving agent can generate unlimited spend.

**Independent Test**: Can be fully tested by running an agent with a BudgetPolicy configured to a low cost limit, feeding it a multi-turn task, and verifying the loop stops with a Stop verdict once the limit is reached.

**Acceptance Scenarios**:

1. **Given** an agent with a budget policy (max cost $1.00) in pre-turn policies, **When** accumulated cost reaches $1.00, **Then** the loop stops gracefully before making the next LLM call, and the stop reason includes the budget policy name and limit.
2. **Given** an agent with no policies in any slot, **When** a multi-turn conversation runs, **Then** the loop runs with no restrictions (anything-goes default).
3. **Given** an agent with a budget policy (max cost $5.00), **When** accumulated cost is $4.99, **Then** the loop continues normally.

---

### User Story 2 - Agent Consumer Controls Tool Access (Priority: P1)

A library consumer deploying an agent in a restricted environment wants to block certain tools from executing and enforce file path sandboxing. They add a tool deny list policy and a sandbox policy to the pre-dispatch slot. The policies evaluate before the user approval gate, so the user never sees tool calls that the system has already rejected.

**Why this priority**: Security-sensitive deployments require system-level tool restrictions that cannot be bypassed by the LLM or user approval.

**Independent Test**: Can be fully tested by configuring an agent with denied tools, triggering the LLM to call a denied tool, and verifying an error result is returned to the LLM without the tool ever executing.

**Acceptance Scenarios**:

1. **Given** an agent with a tool deny list policy blocking "bash" in pre-dispatch policies, **When** the LLM requests a "bash" tool call, **Then** the tool call is skipped with an error message sent back to the LLM, and the tool never executes.
2. **Given** an agent with a sandbox policy (allowed root "/tmp/workspace") in pre-dispatch policies, **When** the LLM requests a file write to "/etc/passwd", **Then** the tool call is skipped with a descriptive error indicating the path is outside the allowed root.
3. **Given** a pre-dispatch policy returns Skip, **When** the tool dispatch continues, **Then** the approval gate is never invoked for that tool call.

---

### User Story 3 - Agent Consumer Stacks Multiple Policies Per Slot (Priority: P1)

A library consumer needs both budget enforcement and a maximum turn limit. They add both policies to the same pre-turn slot. The policies evaluate in the order they were added. If budget stops the loop, the turn limit policy never runs.

**Why this priority**: The composability of multiple policies in a single slot is the core value proposition of the slot system over the old single-hook design.

**Independent Test**: Can be fully tested by adding two policies to the same slot and verifying both are evaluated (or short-circuited) according to the documented semantics.

**Acceptance Scenarios**:

1. **Given** an agent with budget policy then max turns policy in pre-turn policies, **When** the budget limit is reached on turn 3 and max turns is 10, **Then** the loop stops due to budget, not turns.
2. **Given** an agent with budget policy then max turns policy in pre-turn policies, **When** the budget is not reached but turn 10 is reached, **Then** the loop stops due to max turns.
3. **Given** an agent with two policies where the first returns Inject and the second returns Continue, **When** the slot is evaluated, **Then** both policies run and the injected messages are collected.
4. **Given** an agent with two policies where the first returns Stop, **When** the slot is evaluated, **Then** the second policy is never called.

---

### User Story 4 - Agent Consumer Adds Post-Turn Persistence (Priority: P2)

A library consumer wants to checkpoint agent state after every turn for crash recovery. They implement a post-turn policy that persists the context to a store. This replaces the old post-turn hook with a composable policy that can coexist with other post-turn behaviors.

**Why this priority**: Persistence is important but less critical than cost control and security. It's a straightforward migration of existing functionality.

**Independent Test**: Can be fully tested by running a multi-turn agent with a checkpoint policy, verifying the store is written after each turn.

**Acceptance Scenarios**:

1. **Given** an agent with a checkpoint policy in post-turn policies, **When** a turn completes, **Then** the checkpoint store receives the current context state.
2. **Given** an agent with checkpoint policy then max turns policy in post-turn policies, **When** a turn completes, **Then** the checkpoint is persisted before the turn limit is checked.

---

### User Story 5 - Agent Consumer Detects Stuck Loops (Priority: P2)

A library consumer wants to detect when the model is stuck in a cycle, calling the same tool with identical arguments repeatedly. They add a loop detection policy to the post-turn slot that monitors recent turns. When a cycle is detected, the policy either injects a steering message or stops the loop.

**Why this priority**: Loop detection prevents wasted compute and cost from a malfunctioning model. It is a compelling example of a policy that should not be hardcoded but is valuable for many consumers.

**Independent Test**: Can be fully tested by simulating an agent where the LLM repeats the same tool call pattern and verifying the policy fires after the configured lookback window.

**Acceptance Scenarios**:

1. **Given** an agent with a loop detection policy (lookback 3) in post-turn policies, **When** the same tool call with identical arguments appears 3 turns in a row, **Then** the policy returns Stop or Inject (steering message) based on configuration.
2. **Given** varied tool calls across turns, **When** the loop detection policy evaluates, **Then** it returns Continue.

---

### User Story 6 - Custom Policy Implementation (Priority: P3)

A library consumer implements their own policy by creating a struct that implements one of the four slot traits. They receive a policy context, return a verdict, and the loop handles the rest.

**Why this priority**: Extensibility is the long-term value, but the built-in policies cover the most common needs first.

**Independent Test**: Can be fully tested by implementing a minimal custom policy, adding it to a slot, and verifying it is called with the correct context.

**Acceptance Scenarios**:

1. **Given** a custom struct implementing the pre-turn policy trait, **When** added to pre-turn policies, **Then** the policy's evaluate method is called before each LLM call with accurate policy context data.
2. **Given** a custom struct implementing the pre-dispatch policy trait, **When** added to pre-dispatch policies, **Then** the policy can inspect and mutate tool call arguments.

---

### Edge Cases

- What happens when a PreDispatch policy returns Stop (not Skip)? The entire batch is aborted — no tools execute (including any that already passed pre-dispatch earlier in the batch), and the loop stops immediately. Stop means "something is fundamentally wrong, halt now."
- What happens when a PostTurn policy returns Inject but the inner loop was going to break? The injected messages are added to pending and the inner loop continues to process them.
- What happens when all policies return Continue for an empty vec? The slot runner returns Continue (no-op for empty vecs).
- What happens when a PreDispatch policy mutates arguments to something that fails schema validation? The hardcoded schema validation catches it after all policies run and returns an error result to the LLM.
- What happens when a PostLoop policy returns Inject? The injected messages go to pending and the outer loop continues (same as follow-up messages).
- What happens when a PreDispatch policy returns Skip for one tool call in a batch of three? Only that tool call is skipped. The other two proceed through approval and execution normally.
- What happens when a policy's evaluate method panics? The panic is caught via `catch_unwind`, treated as Continue (the panicking policy is skipped), and logged at warn level with the policy name. The loop continues. This matches the existing panic-isolation pattern for event subscribers.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST support four policy slots (PreTurn, PreDispatch, PostTurn, PostLoop), each accepting zero or more policy implementations.
- **FR-002**: System MUST evaluate policies within a slot in the order they appear in the vec (first-in-vec = highest priority).
- **FR-003**: System MUST short-circuit on Stop — once a policy returns Stop, no further policies in that slot are evaluated.
- **FR-004**: System MUST short-circuit on Skip in the PreDispatch slot — once a policy returns Skip for a tool call, no further policies for that tool call are evaluated.
- **FR-005**: System MUST accumulate Inject verdicts — all non-short-circuited policies in a slot get a chance to inject messages.
- **FR-006**: The Skip verdict MUST only be valid in the PreDispatch slot, enforced at compile time. The PreDispatch trait MUST return a `PreDispatchVerdict` enum (Continue, Stop, Inject, Skip). The other three slot traits MUST return a `PolicyVerdict` enum (Continue, Stop, Inject — no Skip variant). This makes misuse a type error, not a runtime surprise.
- **FR-007**: The user approval gate MUST remain separate from the policy system. PreDispatch policies run before approval, not instead of it.
- **FR-008**: JSON Schema validation of tool arguments MUST remain hardcoded in the loop, running after PreDispatch policies and after the approval gate.
- **FR-009**: When all policy slot vecs are empty, the loop MUST run with no restrictions (default anything-goes behavior).
- **FR-010**: Every policy evaluation MUST receive a shared context containing: turn index, accumulated usage, accumulated cost, message count, overflow signal, and a read-only slice of the current conversation messages (`&[AgentMessage]`).
- **FR-011**: PreDispatch policies MUST receive per-tool-call context with tool name, tool call ID, and mutable access to arguments.
- **FR-012**: PostTurn policies MUST receive per-turn context with the assistant message, tool results, and stop reason.
- **FR-013**: Each policy trait MUST include a name method returning a string identifier for tracing and debugging.
- **FR-014**: All four policy traits MUST be synchronous (not async). The evaluate method takes `&self` (immutable reference). Trait bounds are `Send + Sync` only — the slot runner wraps each `evaluate` call with `AssertUnwindSafe` for `catch_unwind`, so implementors do not need to satisfy `UnwindSafe` bounds. Stateful policies MUST use interior mutability (e.g., `Mutex`, `AtomicU64`) for internal state. This preserves `Arc<dyn Policy>` sharing.
- **FR-015**: The following fields MUST be removed from AgentLoopConfig: budget_guard, loop_policy, post_turn_hook, tool_validator, tool_call_transformer.
- **FR-016**: The following fields MUST be added to AgentLoopConfig: pre_turn_policies, pre_dispatch_policies, post_turn_policies, post_loop_policies (all as vecs of trait objects).
- **FR-017**: The slot runner MUST only emit debug-level tracing when a Stop or Skip verdict fires, including the policy name. Normal Continue evaluation MUST be silent.
- **FR-018**: The slot runner MUST catch panics from policy evaluate methods via `catch_unwind` (using `AssertUnwindSafe` wrapper), treat panics as Continue (skip the panicking policy), and log at warn level with the policy name. Panicking policies MUST NOT crash the agent loop.
- **FR-019**: Built-in policy implementations (BudgetPolicy, MaxTurnsPolicy, SandboxPolicy, ToolDenyListPolicy, CheckpointPolicy, LoopDetectionPolicy) MUST be provided as opt-in convenience structs. None MUST be enabled by default.
- **FR-020**: The builder pattern MUST support adding policies individually via dedicated methods that push to the respective vecs.
- **FR-021**: Context transforms (async and sync) MUST remain as separate config fields, not policies.
- **FR-022**: The following loop mechanics MUST remain hardcoded: cancellation token checks, overflow signal handling, retry logic and model fallback, event emission, JSON Schema validation, tool dispatch ordering, context transform invocation, message conversion, and API key resolution.
- **FR-023**: MetricsCollector MUST remain a separate config field, not a policy. It is an observation mechanism (read-only reporting of turn timing, token usage, and cost data) and MUST NOT influence control flow. Policies control the loop; metrics observe it.
- **FR-024**: CheckpointPolicy MUST handle the sync/async boundary by spawning the checkpoint save as a fire-and-forget `tokio::spawn` task and returning Continue immediately. The policy captures a `tokio::runtime::Handle` to spawn onto. Checkpoint persistence MUST NOT block the policy evaluation loop.
- **FR-025**: SandboxPolicy MUST be configured with a list of argument field names to inspect for file paths (default: `["path", "file_path", "file"]`). Only string values in the specified fields are checked against the allowed root. SandboxPolicy MUST Skip with a descriptive error when a path falls outside the allowed root (not silently rewrite).
- **FR-026**: PreDispatch evaluation MUST use a two-pass approach: first, evaluate all PreDispatch policies for all tool calls in the batch (collecting verdicts). If any tool call receives a Stop verdict, the entire batch is aborted before any tool executes. Only after all tool calls pass pre-dispatch does the loop proceed to approval and execution.

### Key Entities

- **PolicyVerdict**: The outcome of a policy evaluation for PreTurn, PostTurn, and PostLoop slots — Continue, Stop (with reason), or Inject (with messages). Does not include Skip.
- **PreDispatchVerdict**: The outcome of a PreDispatch policy evaluation — Continue, Stop (with reason), Inject (with messages), or Skip (with error text). Separate type from PolicyVerdict to enforce Skip-only-in-PreDispatch at compile time.
- **PolicyContext**: Shared read-only context for all policy evaluations — turn state, accumulated usage, accumulated cost, message count, overflow signal, and conversation messages slice.
- **ToolPolicyContext**: Per-tool-call context with mutable access to arguments, available to PreDispatch policies.
- **TurnPolicyContext**: Per-turn context with the assistant message and tool results, available to PostTurn policies.
- **PreTurnPolicy**: Slot 1 trait — evaluated before each LLM call. Guards and pre-conditions.
- **PreDispatchPolicy**: Slot 2 trait — evaluated per tool call before approval and execution. Can mutate arguments.
- **PostTurnPolicy**: Slot 3 trait — evaluated after each completed turn. Reacts to turn results.
- **PostLoopPolicy**: Slot 4 trait — evaluated after the inner loop exits, before follow-up polling.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: All existing agent functionality (budget limiting, turn limiting, tool validation, tool argument transformation, post-turn hooks) can be expressed as policy implementations without loss of capability.
- **SC-002**: An agent constructed with empty policy vecs behaves identically to the current agent with no hooks, guards, or validators configured.
- **SC-003**: Adding a policy to a slot requires implementing a single trait method and adding it to the builder. No other code changes are needed.
- **SC-004**: Policy evaluation adds negligible overhead — evaluating an empty policy vec requires no allocation.
- **SC-005**: A consumer can compose multiple policies in a single slot and the evaluation semantics (Stop short-circuits, Inject accumulates) are deterministic and match documented ordering.
- **SC-006**: The AgentLoopConfig has fewer top-level fields after migration (4 policy vecs replace 5+ individual option fields).
- **SC-007**: All built-in policy implementations pass unit tests demonstrating correct verdict generation for their respective scenarios.

## Clarifications

### Session 2026-03-24

- Q: What should happen when a policy's evaluate method panics? → A: Catch via catch_unwind, treat as Continue (skip the panicking policy), log at warn level with policy name. Matches existing subscriber panic-isolation pattern.
- Q: How do stateful policies (e.g., LoopDetectionPolicy) maintain state with `&self`? → A: Interior mutability is the implementor's responsibility. Trait takes `&self`; stateful policies use `Mutex<State>` or atomics internally. Standard Rust pattern, preserves `Arc<dyn Policy>` sharing.
- Q: What happens when a PreDispatch policy returns Stop mid-batch (e.g., tool 2 of 3)? → A: Entire batch is aborted — no tools execute, including any that already passed pre-dispatch. Loop stops immediately.
- Q: How is "Skip only valid in PreDispatch" (FR-006) enforced? → A: Compile-time via separate verdict types. PreDispatch returns `PreDispatchVerdict` (has Skip); other slots return `PolicyVerdict` (no Skip). Type system prevents misuse.
- Q: Should policy traits require UnwindSafe bounds for catch_unwind? → A: No. The slot runner uses `AssertUnwindSafe` wrapper. Traits only need `Send + Sync`. This avoids burdening implementors with UnwindSafe bounds that are hard to satisfy with interior mutability.
- Q: How does CheckpointPolicy call async CheckpointStore from a sync evaluate method? → A: Fire-and-forget via `tokio::spawn`. CheckpointPolicy captures a `tokio::runtime::Handle` and spawns the save task. Returns Continue immediately without blocking.
- Q: How does SandboxPolicy know which argument fields contain file paths? → A: Configured with a list of field names to check (default: `["path", "file_path", "file"]`). Only string values in those fields are validated. Skip with error on violation (no silent rewriting).
- Q: Is PreDispatch evaluation per-tool sequential or two-pass? → A: Two-pass. First pass evaluates all PreDispatch policies for all tool calls. If any returns Stop, entire batch is aborted before any tool executes. Second pass proceeds to approval and execution for all passing tool calls.
- Q: Should PolicyContext include conversation messages? → A: Yes. Added `messages: &[AgentMessage]` to PolicyContext (FR-010) so PreTurn policies can inspect message history. Required by 032-policy-recipes-crate (PromptInjectionGuard needs to scan user messages). Backward-compatible — existing policies simply ignore the new field.
