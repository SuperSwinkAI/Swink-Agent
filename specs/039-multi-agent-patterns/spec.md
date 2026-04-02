# Feature Specification: Multi-Agent Patterns Crate & Pipeline Primitives

**Feature Branch**: `039-multi-agent-patterns`  
**Created**: 2026-04-02  
**Status**: Draft  
**Input**: User description: "Multi-Agent Patterns Crate & Pipeline Primitives — new `swink-agent-patterns` workspace crate housing pipeline types, execution engine, registry, and future multi-agent composition patterns (transfer, swarm, etc.)"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Consumer Chains Agents in a Sequential Pipeline (Priority: P1)

A library consumer building a content production system wants three agents to collaborate in sequence: a research agent gathers information, an analysis agent synthesizes findings, and a writing agent produces the final output. Today they must manually wire the output of each agent into the next with custom glue code — extracting the response, formatting it as a user message, and feeding it to the next agent. With sequential pipelines, they define the chain declaratively and the executor handles message passing between steps automatically.

**Why this priority**: Sequential composition is the most common multi-agent pattern. Without it, consumers must write repetitive boilerplate to chain agents — exactly the kind of plumbing a patterns library should eliminate.

**Independent Test**: Can be fully tested by registering two agents in an `AgentRegistry`, creating a sequential pipeline referencing them by name, executing it with an input string, and verifying the second agent receives the first agent's output and the pipeline returns the second agent's response.

**Acceptance Scenarios**:

1. **Given** a sequential pipeline with steps [agent-A, agent-B], **When** executed with input "Hello", **Then** agent-A receives "Hello" as a user message, agent-B receives agent-A's final response as a user message, and the pipeline returns agent-B's response.
2. **Given** a sequential pipeline with three steps, **When** agent-B (step 2) errors, **Then** the pipeline halts immediately, agent-C never runs, and the pipeline returns a step-failure error identifying step 2 and agent-B.
3. **Given** a sequential pipeline with `pass_context: true`, **When** executed, **Then** each step receives the full conversation history from all prior steps, not just the previous step's final response.
4. **Given** a sequential pipeline referencing agent "missing" by name, **When** the executor resolves agents at runtime, **Then** the pipeline fails with an agent-not-found error before any step executes.

---

### User Story 2 - Consumer Fans Out Work to Parallel Agents (Priority: P1)

A library consumer building a multi-perspective analysis tool wants several specialized agents to analyze the same input simultaneously — a sentiment agent, a factual-accuracy agent, and a style agent. Today they must manually spawn concurrent tasks, collect results, and merge them. With parallel pipelines, they declare the branches and a merge strategy, and the executor handles concurrency and result aggregation.

**Why this priority**: Parallel composition is the second most common pattern and delivers the highest throughput improvement. It directly leverages async concurrency and complements sequential pipelines — together they cover the majority of real-world multi-agent workflows.

**Independent Test**: Can be fully tested by registering three agents, creating a parallel pipeline with Concat merge strategy, executing it, and verifying all three agents received the same input and their outputs are concatenated in the result.

**Acceptance Scenarios**:

1. **Given** a parallel pipeline with branches [agent-A, agent-B, agent-C] and Concat merge strategy, **When** executed with input "Analyze this", **Then** all three agents receive "Analyze this" concurrently and the pipeline output contains all three responses joined by a separator.
2. **Given** a parallel pipeline with First merge strategy, **When** agent-B completes before agents A and C, **Then** the pipeline returns agent-B's response and cancels the remaining branches.
3. **Given** a parallel pipeline with Fastest(2) merge strategy and three branches, **When** two branches complete, **Then** the pipeline returns those two responses and cancels the remaining branch.
4. **Given** a parallel pipeline where the pipeline's cancellation token is cancelled mid-execution, **Then** all running branches are cancelled and the pipeline returns a cancelled error.

---

### User Story 3 - Consumer Runs an Agent in a Loop Until a Condition Is Met (Priority: P1)

A library consumer building a self-correcting code generation system wants an agent to repeatedly refine its output until it passes validation. The agent generates code, a validation tool checks it, and if it fails, the agent tries again with the error feedback. Today the consumer must build this retry loop manually. With loop pipelines, they declare the body agent, an exit condition (e.g., a specific tool was called indicating success), and a safety cap on iterations.

**Why this priority**: Iterative refinement is the third fundamental composition pattern. It enables agentic loops within agentic loops — critical for self-correcting, reflective, and multi-pass workflows that are increasingly common in production agent systems.

**Independent Test**: Can be fully tested by registering an agent that calls a "done" tool on its third invocation, creating a loop pipeline with `ToolCalled("done")` exit condition and max_iterations of 5, and verifying the loop runs exactly 3 iterations.

**Acceptance Scenarios**:

1. **Given** a loop pipeline with exit condition `ToolCalled("validate_pass")` and max_iterations 10, **When** the body agent calls "validate_pass" on iteration 3, **Then** the loop exits after iteration 3 and returns the agent's output from that iteration.
2. **Given** a loop pipeline with exit condition `OutputContains("DONE")` and max_iterations 5, **When** the agent outputs "Task DONE" on iteration 2, **Then** the loop exits after iteration 2.
3. **Given** a loop pipeline with max_iterations 3 and an exit condition that is never met, **When** all 3 iterations complete, **Then** the pipeline returns a max-iterations-reached error.
4. **Given** a loop pipeline where the body agent errors on iteration 2, **Then** the loop halts immediately with a step-failure error.

---

### User Story 4 - Consumer Registers and Manages Pipeline Definitions (Priority: P1)

A library consumer building a multi-pipeline application needs to register several pipeline definitions, look them up by ID, list available pipelines, and remove ones no longer needed. The pipeline registry provides the same ergonomic, thread-safe storage pattern they already use for agents via `AgentRegistry`.

**Why this priority**: The registry is the lookup mechanism the executor depends on. Without it, every pipeline execution would require passing the full pipeline definition inline. The registry enables named, reusable pipeline definitions.

**Independent Test**: Can be fully tested by creating a registry, registering three pipelines, verifying list returns all three, looking up by ID, removing one, and verifying it's gone.

**Acceptance Scenarios**:

1. **Given** an empty pipeline registry, **When** the consumer registers a sequential pipeline named "content-chain", **Then** the registry returns a `PipelineId` and `get(id)` returns the pipeline.
2. **Given** a registry with three pipelines, **When** the consumer calls `list()`, **Then** all three are returned as (id, name) pairs.
3. **Given** a registered pipeline with a known ID, **When** the consumer calls `remove(id)`, **Then** subsequent `get(id)` returns None.
4. **Given** a pipeline ID that doesn't exist, **When** the consumer calls `get(id)`, **Then** None is returned without error.

---

### User Story 5 - Consumer Exposes a Pipeline as a Tool for Other Agents (Priority: P2)

A library consumer wants a "supervisor" agent to be able to invoke a pipeline as one of its tools — for example, an orchestrator agent can call a "research-pipeline" tool that triggers a three-step sequential pipeline behind the scenes. This bridges the tool system and the pipeline system, enabling hierarchical compositions where agents can delegate to entire pipelines.

**Why this priority**: Pipeline-as-tool is a powerful composition primitive but depends on pipelines already working (US1-US4). It extends the system's expressiveness but is not required for basic pipeline usage.

**Independent Test**: Can be fully tested by wrapping a sequential pipeline as a `PipelineTool`, adding it to a supervisor agent's tool list, invoking the supervisor, and verifying the pipeline executes and returns its result as the tool output.

**Acceptance Scenarios**:

1. **Given** a `PipelineTool` wrapping pipeline "research-chain", **When** the supervisor agent calls the tool with input arguments, **Then** the pipeline executes and the tool returns the pipeline's final response as its result text.
2. **Given** a `PipelineTool` whose underlying pipeline errors, **When** the tool executes, **Then** the tool returns an error result with the pipeline error details (not a panic or crash).
3. **Given** a `PipelineTool`, **When** inspected for its schema, **Then** it exposes an input parameter for the pipeline input string and a description derived from the pipeline name.

---

### User Story 6 - Consumer Uses a Custom Aggregator Agent for Parallel Results (Priority: P2)

A library consumer running a parallel pipeline wants more than simple concatenation — they want an aggregator agent to synthesize the parallel results into a coherent summary. They configure the parallel pipeline with a Custom merge strategy that names an aggregator agent. After all branches complete, the executor passes all branch outputs to the aggregator agent, which produces the final synthesized response.

**Why this priority**: Custom merge via an aggregator agent is the most flexible merge strategy but is an advanced use case. Most consumers will start with Concat or First. This extends parallel pipelines for sophisticated workflows.

**Independent Test**: Can be fully tested by registering an aggregator agent and three branch agents, creating a parallel pipeline with Custom merge strategy naming the aggregator, and verifying the aggregator receives all three branch outputs and produces the final result.

**Acceptance Scenarios**:

1. **Given** a parallel pipeline with Custom merge strategy naming agent "synthesizer", **When** all branches complete, **Then** the synthesizer agent receives all branch outputs as a single user message with labeled sections (`[agent-name]: output` separated by blank lines) and the pipeline returns the synthesizer's response.
2. **Given** a Custom merge strategy referencing agent "missing", **When** all branches complete, **Then** the pipeline fails with an agent-not-found error for the aggregator.

---

### User Story 7 - Consumer Observes Pipeline Execution via Events (Priority: P2)

A library consumer building a monitoring dashboard wants to track pipeline execution progress in real time — when a pipeline starts, when each step begins and completes, and when the overall pipeline finishes or fails. Pipeline events are emitted through the existing `AgentEvent` system so the consumer's existing event listeners and forwarders receive them without additional wiring.

**Why this priority**: Observability is important for production use but is not required for pipelines to function. It integrates with the existing event infrastructure, making it a natural extension.

**Independent Test**: Can be fully tested by registering an event listener, executing a sequential pipeline, and verifying the listener received PipelineStarted, PipelineStepStarted, PipelineStepCompleted (for each step), and PipelineCompleted events in order.

**Acceptance Scenarios**:

1. **Given** an event listener registered on the executor, **When** a sequential pipeline with two steps executes successfully, **Then** the listener receives events in order: PipelineStarted, PipelineStepStarted(step 0), PipelineStepCompleted(step 0), PipelineStepStarted(step 1), PipelineStepCompleted(step 1), PipelineCompleted.
2. **Given** a pipeline that fails at step 1, **When** the executor emits events, **Then** PipelineFailed is emitted with the pipeline ID and error details.
3. **Given** PipelineStepCompleted events, **When** inspected, **Then** each carries the step's agent name, response, duration, and token usage.

---

### Edge Cases

- What happens when a sequential pipeline has zero steps? The pipeline returns immediately with an empty response and zero usage. This is valid but unusual — the consumer is not prevented from creating degenerate pipelines.
- What happens when a parallel pipeline has one branch? It executes as a single concurrent branch — functionally equivalent to a sequential pipeline with one step, but the parallel execution machinery still runs. No special-casing.
- What happens when a loop pipeline's body agent returns no text output (only tool calls)? The exit condition check for `OutputContains` matches against an empty string. `ToolCalled` checks the tool execution history. If neither condition matches and the iteration cap isn't reached, the loop continues.
- What happens when the executor is asked to run a pipeline whose agents have been removed from the `AgentRegistry` between registration and execution? The executor fails with an agent-not-found error at the step that references the missing agent. Steps that already completed are not rolled back.
- What happens when a parallel pipeline branch panics or errors? The panic is caught by the task boundary. The branch is treated as a failed step. For Concat, the entire pipeline fails (strict — no partial results). For First/Fastest, remaining branches may still satisfy the merge strategy if enough succeed.
- What happens when a loop pipeline's exit condition uses a regex that fails to compile? The regex is compiled at pipeline construction time (not at execution time). Construction fails with an error if the regex is invalid.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST provide a `swink-agent-patterns` workspace crate at `patterns/` that depends only on `swink-agent` public API and contains no internal imports from the core crate.
- **FR-002**: System MUST provide three pipeline types — Sequential, Parallel, and Loop — as variants of a Pipeline sum type.
- **FR-003**: Sequential pipelines MUST execute agents in declared order, passing each agent's final text output (concatenated text content blocks only, excluding tool call metadata) as the next agent's user message input.
- **FR-004**: Sequential pipelines MUST support a `pass_context` flag that, when true, passes user and assistant text messages from all prior steps to each subsequent step (tool call/result pairs are stripped). When false, only the previous step's final text output is passed.
- **FR-005**: Parallel pipelines MUST execute all branches concurrently, with each branch receiving a child cancellation token derived from the pipeline's token.
- **FR-006**: Parallel pipelines MUST support four merge strategies: Concat (join all outputs in declaration order for deterministic results), First (return first completed), Fastest(N) (return first N completed), and Custom (pass all outputs to an aggregator agent).
- **FR-007**: First and Fastest merge strategies MUST cancel remaining branches once their completion criteria are met.
- **FR-008**: Loop pipelines MUST execute a body agent repeatedly until an exit condition is met or a mandatory `max_iterations` safety cap is reached. On iteration 1, the agent receives the original pipeline input. On iteration 2+, the agent receives the original input plus conversation history from all prior iterations (accumulating context).
- **FR-009**: Loop pipelines MUST support three exit conditions: ToolCalled (specific tool name was invoked), OutputContains (regex match on agent output), and MaxIterations (always run to the cap).
- **FR-010**: Loop pipeline regex exit conditions MUST be compiled at pipeline construction time, failing eagerly on invalid patterns.
- **FR-011**: All pipeline types MUST resolve agents by name from the `AgentRegistry` at execution time, not at pipeline construction time.
- **FR-012**: System MUST provide a `PipelineId` newtype for pipeline identity.
- **FR-013**: System MUST provide a `PipelineRegistry` for in-memory pipeline storage with `new()`, `register()`, `get()`, `list()`, and `remove()` methods, using thread-safe interior mutability. Registering a pipeline with the same ID as an existing entry MUST silently replace the previous entry (consistent with `AgentRegistry` behavior).
- **FR-014**: System MUST provide a stateless `PipelineExecutor` that coordinates pipeline execution using agent and pipeline registries, exposing a `run()` method accepting a pipeline ID, input string, and cancellation token. The executor MUST accept an optional event channel/callback at construction time for emitting pipeline events. When no event channel is provided, event emission is a no-op with zero overhead.
- **FR-015**: Pipeline execution MUST produce a `PipelineOutput` containing the pipeline ID, final response string, per-step results (agent name, response, duration, usage), total duration, and total aggregated usage.
- **FR-016**: Pipeline execution MUST fail with typed `PipelineError` variants: AgentNotFound, PipelineNotFound, StepFailed (with step index, agent name, and source error), MaxIterationsReached, and Cancelled.
- **FR-017**: System MUST provide a `PipelineTool` wrapper that implements the agent tool trait, allowing any pipeline to be exposed as a tool for other agents.
- **FR-018**: Pipeline execution MUST emit events through the existing event system: PipelineStarted, PipelineStepStarted, PipelineStepCompleted, PipelineCompleted, and PipelineFailed.
- **FR-019**: Pipeline types MUST be serializable and deserializable for persistence by downstream consumers.
- **FR-020**: The pipeline module MUST be feature-gated under a `pipelines` feature flag (default-enabled). Future pattern modules MUST get their own independent feature gates.
- **FR-021**: If any step in a sequential or loop pipeline errors, execution MUST halt immediately — no subsequent steps run.
- **FR-022**: The crate MUST be added to the workspace members with its dependencies centralized in workspace dependency management.

### Key Entities

- **Pipeline**: The composition definition — a named, typed description of how multiple agents collaborate. Variants: Sequential (ordered chain), Parallel (concurrent fan-out), Loop (iterative refinement). The core abstraction of the crate.
- **PipelineId**: Unique identifier for a registered pipeline definition. The identity type that makes pipelines addressable in a registry.
- **PipelineRegistry**: Thread-safe container for pipeline definitions. Provides registration, lookup, listing, and removal. The storage layer that enables named, reusable pipelines.
- **PipelineExecutor**: Stateless coordinator that resolves agents and pipelines from their respective registries and drives execution. The runtime engine.
- **PipelineOutput**: Structured result of a pipeline execution — contains the final response, per-step telemetry (agent name, response text, timing, token usage), and aggregated totals.
- **PipelineTool**: Bridge between the pipeline system and the tool system — wraps a pipeline as a tool so agents can invoke pipelines as tools in their turns.
- **MergeStrategy**: Controls how parallel pipeline branch outputs are combined — Concat, First, Fastest(N), or Custom (delegating to an aggregator agent).
- **ExitCondition**: Controls when a loop pipeline terminates — ToolCalled, OutputContains (regex), or MaxIterations.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Library consumers can define a sequential pipeline with N agents and execute it with a single call, eliminating manual output-to-input wiring between agents.
- **SC-002**: Parallel pipelines achieve concurrent execution — wall-clock time for a parallel pipeline with N branches is closer to the slowest branch than to the sum of all branches.
- **SC-003**: Loop pipelines terminate deterministically — every loop pipeline either meets its exit condition or hits its max_iterations cap. No unbounded loops are possible.
- **SC-004**: Pipeline definitions are reusable — the same pipeline can be executed multiple times with different inputs without re-registration.
- **SC-005**: Pipeline-as-tool composition works end-to-end — an agent can invoke a pipeline via the tool system and receive the pipeline's output as a tool result.
- **SC-006**: Pipeline events integrate seamlessly with existing event infrastructure — consumers with existing event listeners receive pipeline events without additional configuration.
- **SC-007**: The patterns crate compiles and tests independently — testing the crate in isolation passes without requiring any downstream consumer crates.
- **SC-008**: Consumers who do not use the patterns crate incur zero compile-time or runtime cost — it is a separate optional dependency.

## Assumptions

- The patterns crate is the designated home for all higher-level multi-agent composition patterns. Pipelines are the first feature; future additions (agent transfer/handoff, swarm patterns, graph-based orchestration) will be added as independently feature-gated modules in this same crate.
- Agents are resolved by name from the `AgentRegistry` at execution time (late binding). This enables hot-swapping agent implementations between pipeline runs and avoids holding stale agent handles in pipeline definitions.
- The `PipelineExecutor` is intentionally stateless — it does not cache agents, track execution history, or maintain session state. All state lives in the registries and agents themselves. This makes the executor safe to share across threads and simple to reason about.
- Pipeline definitions are data-only structures (no closures, no trait objects) that are fully serializable. This enables persistence, transmission, and inspection of pipeline configurations by downstream consumers.
- The Custom merge strategy for parallel pipelines uses an agent (by name) as the aggregator, not a closure. This keeps pipeline definitions serializable and consistent with the "everything resolved by name" pattern.
- Pipeline events are emitted as custom event variants through the existing event system. This avoids modifying the core event enum for a feature that lives in a separate crate.
- The `PipelineTool` wrapper creates a simple schema with a single input string parameter. More complex tool schemas (multiple parameters, structured input) can be added later via builder methods.
- In multi-agent scenarios, pipelines reference agents from a shared `AgentRegistry`. The registry must be populated before pipeline execution. Pipelines do not create or configure agents — they only compose existing ones.
- `PipelineId` uses a string-based identity because pipeline IDs benefit from being human-readable and may be serialized/deserialized across sessions. Each pipeline carries its own ID set at construction time (auto-generated UUID if not provided by the consumer). The registry uses the pipeline's ID as its key — `register(pipeline)` reads the ID from the pipeline, it does not assign one.
- The Loop pipeline's `ToolCalled` exit condition checks whether the body agent invoked a tool with the specified name during the most recent iteration. It does not inspect tool results — only tool invocation.
- Pipeline steps reference agents only (by name), not other pipelines. Pipeline nesting is achieved via the `PipelineTool` wrapper (US5) — wrap a sub-pipeline as a tool on a relay agent. This keeps the step data model simple and serializable while still enabling hierarchical composition.
- All pipeline execution uses fresh agent instances per step/branch, not shared `AgentRef` handles. The executor locks the registered `AgentRef`, clones the agent's configuration, and constructs a new `Agent` from it — the registered agent serves as a template. This makes pipeline execution stateless and repeatable — running the same pipeline twice produces the same result regardless of prior runs. For parallel branches this also avoids mutex contention.

## Clarifications

### Session 2026-04-02

- Q: Can pipeline steps reference other pipelines (nesting)? → A: No. Steps are agent-names-only; nesting is achieved via PipelineTool wrapper.
- Q: What happens when parallel branches reference the same agent? → A: Each branch gets a fresh agent instance (cloned config), ensuring true parallelism with no mutex contention.
- Q: What input does the loop body agent receive on iteration 2+? → A: Original input plus conversation history from all prior iterations (accumulating context).
- Q: How does the executor emit pipeline events without holding an Agent? → A: Executor accepts an optional event channel/callback at construction; no-op when absent.
- Q: What content passes between sequential steps? → A: Text content only (concatenated text blocks); tool call metadata is excluded.
- Q: Pipeline name uniqueness in the registry? → A: Replace silently on collision, matching AgentRegistry behavior.
- Q: Should parallel Concat fail if any branch fails? → A: Yes, strict failure — no partial results. Best-effort can be a future separate strategy.
- Q: Does pass_context include tool call/result messages? → A: No, user and assistant text messages only; tool call/result pairs are stripped.
- Q: Who provides the PipelineId? → A: Pipeline carries its own ID (set at construction, auto-UUID if not provided); registry uses it as key.
- Q: Should sequential pipelines also use fresh agent instances? → A: Yes, fresh instances for all pipeline types. Execution is stateless and repeatable.
- Q: How does the executor create fresh agent instances? → A: Locks the registered AgentRef, clones the config, constructs a new Agent from it. No registry API changes.
- Q: How are branch outputs formatted for the Custom merge aggregator? → A: Labeled text sections — `[agent-name]: output` separated by blank lines — in a single user message.
- Q: Concat merge — declaration order or completion order? → A: Declaration order for deterministic, predictable output.
