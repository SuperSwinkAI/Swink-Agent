# Research: Self-Improvement Loop

**Date**: 2026-04-27 | **Status**: Complete

## Eval Crate Integration Surface

**Decision**: `swink-agent-evolve` depends on `swink-agent-eval` with features `judge-core` + `telemetry` (optional). All required types are re-exported from `swink-agent-eval::*`.
**Rationale**: Verified the eval crate exports: `EvalRunner`, `EvalCase`, `EvalSet`, `EvalCaseResult`, `EvalSetResult`, `EvalSummary`, `Invocation`, `TurnRecord`, `Score`, `Verdict`, `EvaluatorRegistry`, `JudgeClient`, `JudgeVerdict`, `Reporter`, `ReporterOutput`, `AgentFactory`, `TrajectoryCollector`. The `AgentFactory` trait allows wrapping to swap system prompts and tool schemas per candidate.
**Alternatives considered**: Direct `StreamFn` usage — rejected because the eval runner already handles trajectory collection, budget enforcement, and parallelism.

## AgentFactory Wrapping Strategy

**Decision**: `swink-agent-evolve` provides a `MutatingAgentFactory` that wraps the caller's `AgentFactory` and overrides the system prompt and/or tool schemas before delegating to `create_agent()`.
**Rationale**: The `AgentFactory::create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken)>` signature gives full access to `Agent` construction. The wrapper can intercept `AgentOptions` to swap the system prompt or modify tool schemas before the inner factory builds the agent. This requires the inner factory to expose the options construction (or the wrapper reconstructs options from the `EvalCase` + mutation).
**Alternatives considered**: Patching `EvalCase` fields directly — rejected because `EvalCase` represents the test scenario, not the agent configuration. Mutating the case would conflate test definition with optimization target.

## System Prompt Section Parsing

**Decision**: Parse markdown headers (`## Section Name`) as section boundaries. Unstructured prompts (no headers) are treated as a single section. Users can override with custom delimiters via `OptimizationTarget::with_section_delimiter(regex)`.
**Rationale**: Markdown headers are the most common structuring convention in system prompts. The parser needs to be best-effort — the evolve crate must work with any prompt format, not just well-structured ones.
**Alternatives considered**: XML tags (`<section>`) — less common in practice. Line-based splitting — too granular, produces meaningless mutations. User-defined JSON schema — too rigid, requires prompt restructuring.

## Mutation Strategy: LLM-Guided Rewrite

**Decision**: Use the existing `JudgeClient` trait to send structured prompts requesting rewrites. The mutation prompt template includes: the original text, the failing eval case description, the expected behavior, the actual score, and an instruction to produce an improved version.
**Rationale**: `JudgeClient::judge(prompt) -> JudgeVerdict` is designed for single-prompt-in, structured-response-out. The verdict's `reason` field carries the rewritten text. This reuses existing retry/timeout infrastructure from the judge registry.
**Alternatives considered**: Raw `StreamFn` call — would require building message arrays, handling streaming, and duplicating retry logic. Custom LLM trait — unnecessary abstraction when `JudgeClient` fits.

## Mutation Strategy: Template-Based

**Decision**: Ship a built-in library of ~10 phrasing transformation templates. Each template is a find-replace pair with optional regex support: e.g., imperative→declarative ("You must X" → "X is required"), verbose→concise (remove filler words), restructure (bullet list → prose, prose → bullet list).
**Rationale**: Template mutations are deterministic, zero-cost, and fast. They provide a baseline of candidates even when the judge model is unavailable or the budget is exhausted. The library is extensible via `Mutator::with_template(find, replace)`.
**Alternatives considered**: NLP-based paraphrasing — requires an additional model dependency. Random word substitution — produces low-quality candidates.

## Cost Budget Implementation

**Decision**: Use a shared `CycleBudget` accumulator passed to all phases. Each phase reports its cost via `Usage` and `Cost` types from the eval crate. LLM-guided mutations report cost from `JudgeVerdict`. Eval runs report cost from `EvalSetResult::summary`. The budget is checked before each LLM call and before each candidate evaluation.
**Rationale**: The eval crate already tracks `total_cost` in `EvalSummary`. The evolve crate aggregates across phases without implementing its own token counting.
**Alternatives considered**: Per-phase budgets — more granular but harder for users to configure. Time-based budgets — less predictable and harder to reason about.

## Candidate Deduplication

**Decision**: Hash candidate values with SHA-256 (via `sha2` crate, already a workspace dependency) and deduplicate before evaluation.
**Rationale**: Two strategies may produce identical text (e.g., ablation simplification matches a template transformation). Evaluating duplicates wastes budget. SHA-256 is already used in the eval crate for cache keys.
**Alternatives considered**: String equality — works but slower for large prompts. Semantic similarity — too expensive as a deduplication check.

## Output Directory Structure

**Decision**: `{output_root}/cycle-{number:04}-{iso8601}/` with `manifest.jsonl` and modified configuration files. Configuration files are written as standalone text files (e.g., `system-prompt.md`, `tool-{name}.json`).
**Rationale**: Versioned subdirectories allow multiple cycles to coexist. Zero-padded cycle numbers sort correctly. ISO 8601 timestamps provide human-readable ordering. Standalone files are easier to diff and review than embedded fields in JSON.
**Alternatives considered**: Single manifest with embedded values — harder to diff. Git-based versioning — overengineered for a library crate.

## Observability Integration

**Decision**: Use `tracing::instrument` on each phase method. Feature-gate behind `otel` to match the eval crate pattern. Span attributes include cycle number, phase name, candidate count, and cost.
**Rationale**: The eval crate already sets this pattern with `evaluate_instrumented()`. Adding spans to the evolve crate makes optimization cycles visible in the same tracing backend.
**Alternatives considered**: Custom metrics emission — more work, less standard. Log-only — loses structured trace correlation.
