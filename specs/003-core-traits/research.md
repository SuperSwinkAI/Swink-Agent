# Research: Core Traits

**Feature**: 003-core-traits
**Date**: 2026-03-20

## Technical Context Resolution

No NEEDS CLARIFICATION items. All decisions determined by the PRD
(§4, §7, §11), HLD core abstractions layer, AGENTS.md lessons learned,
and clarification session.

## Decisions

### Tool Trait Design

- **Decision**: Object-safe trait with `name()`, `label()`, `description()`,
  `parameters_schema()`, and async `execute()`. Used as `Arc<dyn AgentTool>`.
- **Rationale**: Object safety required because `AgentContext` stores
  `Vec<Arc<dyn AgentTool>>` (feature 002). The trait surface matches
  PRD §4.1 exactly.
- **Alternatives considered**: Generic tool type parameter — rejected
  because it would propagate through the entire API.

### Tool Argument Validation

- **Decision**: `jsonschema` crate for JSON Schema validation. Validation
  runs before `execute()`. Invalid arguments produce field-level error
  strings without invoking execute.
- **Rationale**: Constitution principle IV (leverage the ecosystem).
  `jsonschema` is the standard Rust JSON Schema validation crate.
- **Alternatives considered**: Manual validation — rejected per
  constitution. `valico` — rejected because `jsonschema` has better
  maintenance and API.

### Tool Result Structure

- **Decision**: `AgentToolResult` with `content: Vec<ContentBlock>`,
  `details: Option<Value>`, and `is_error: bool`. Convenience constructors
  `text()`, `error()`.
- **Rationale**: PRD §4.2 specifies content + details. The `is_error` flag
  replaces the old `text.starts_with("error")` heuristic (AGENTS.md lesson).
- **Alternatives considered**: Separate success/error result types —
  rejected because the LLM always receives content blocks regardless of
  success/failure.

### StreamFn Trait Design

- **Decision**: Object-safe async trait returning `Pin<Box<dyn Stream<Item = AssistantMessageEvent>>>`.
  Accepts `ModelSpec`, `AgentContext`, `StreamOptions`, `CancellationToken`.
- **Rationale**: The sole provider boundary per constitution principle V.
  Box<dyn Stream> keeps the trait object-safe.
- **Alternatives considered**: Generic associated type — rejected because
  it breaks object safety.

### Delta Accumulation

- **Decision**: `accumulate_message()` function that consumes an event
  stream and produces a finalized `AssistantMessage`. Strict ordering
  enforced: one Start, indexed content blocks, one terminal (Done/Error).
  Out-of-order events return an error and terminate. Empty streams produce
  "no Start event found" error.
- **Rationale**: AGENTS.md lessons learned document this exact behavior.
  Clarification session confirmed strict enforcement (no silent recovery).
- **Alternatives considered**: Best-effort reorder — rejected per
  clarification (would mask adapter bugs).

### Default Retry Strategy

- **Decision**: Exponential backoff with configurable max_attempts (3),
  base_delay (1s), max_delay (60s), multiplier (2.0), and jitter ([0.5, 1.5)
  range). Retries only `ModelThrottled` and `NetworkError`.
- **Rationale**: PRD §11 and AGENTS.md lessons learned specify these
  exact parameters. `RetryStrategy::should_retry()` is the sole
  retryability decision point.
- **Alternatives considered**: Per-error retry configuration — rejected
  because the trait's `should_retry` already allows custom logic.
