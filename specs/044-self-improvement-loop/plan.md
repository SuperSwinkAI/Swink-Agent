# Implementation Plan: Eval-Driven Self-Improvement Loop

**Branch**: `044-self-improvement-loop` | **Date**: 2026-04-27 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/044-self-improvement-loop/spec.md`

## Summary

New workspace crate `swink-agent-evolve` implementing a closed-loop optimization cycle for agent system prompts and tool descriptions. The loop: baseline evaluation → weakness diagnosis → candidate mutation (LLM-guided, template-based, ablation) → candidate re-evaluation → quality-gated acceptance → versioned persistence with JSONL audit trail. Builds entirely on `swink-agent-eval`'s `EvalRunner`, `AgentFactory`, `JudgeClient`, and `Reporter` infrastructure. Feature-gated, cost-bounded, deterministic where configured.

## Technical Context

**Language/Version**: Rust latest stable (edition 2024)
**Primary Dependencies**: `swink-agent` (core types — `ToolSchema`, `Agent`, `AgentOptions`, `Cost`, `Usage`), `swink-agent-eval` (eval runner, trajectory, scoring, judges), `sha2` (candidate deduplication), `serde`/`serde_json` (manifest serialization), `regex` (section parsing, template mutations), `tracing` (observability spans), `chrono` (manifest timestamps)
**Storage**: Local filesystem — JSONL manifests + text files for improved configurations
**Testing**: `cargo test --workspace` — unit tests per module, integration test with mock judge and eval set
**Target Platform**: Cross-platform library (any target supporting swink-agent-eval)
**Project Type**: Library crate (`swink-agent-evolve`)
**Performance Goals**: Mutation and gating phases complete in under 1s for typical inputs (excluding LLM calls). Cycle overhead beyond eval runs < 5%.
**Constraints**: `#[forbid(unsafe_code)]`; depends only on `swink-agent` and `swink-agent-eval` public APIs; no direct LLM calling (reuses `JudgeClient`)
**Scale/Scope**: New crate — ~8 modules, ~300-500 lines each. Plus `lib.rs` re-exports.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | PASS | New workspace crate `swink-agent-evolve`. Self-contained, independently compilable, independently testable. Depends only on public APIs of existing crates. |
| II. Test-Driven Development | PASS | Unit tests per module. Integration test with mock `JudgeClient` and `AgentFactory`. TDD: tests written before implementation. |
| III. Efficiency & Performance | PASS | Candidate deduplication via SHA-256 avoids redundant eval runs. Budget propagation prevents runaway costs. Hard caps (FR-023) bound combinatorics. |
| IV. Leverage the Ecosystem | PASS | Reuses `EvalRunner` for all evaluation, `JudgeClient` for LLM-guided mutations, `sha2` for hashing, `regex` for section parsing. No hand-rolled equivalents. |
| V. Provider Agnosticism | PASS | Zero provider-specific code. `JudgeClient` is a trait — any provider's judge implementation works. |
| VI. Safety & Correctness | PASS | `#[forbid(unsafe_code)]`. Panic isolation for mutation strategies. Budget enforcement prevents cost overrun. P1 regression gate prevents quality degradation. |

**Crate count**: Adding a 16th workspace member.

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| 16th workspace crate | Evolution logic has distinct dependencies (mutation templates, section parsing) and a distinct lifecycle (runs after eval, not during). Embedding in eval crate would conflate evaluation with optimization. | Putting evolution in `eval/src/evolve/` would make the eval crate responsible for mutation strategies, template libraries, and configuration persistence — concerns orthogonal to evaluation. |

## Project Structure

### Documentation (this feature)

```text
specs/044-self-improvement-loop/
├── plan.md              # This file
├── spec.md              # Feature specification
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/
│   └── public-api.md    # Phase 1 output
└── checklists/
    └── requirements.md  # Spec quality checklist
```

### Source Code (repository root)

```text
evolve/                              # NEW crate: swink-agent-evolve
├── Cargo.toml                       # workspace member, feature gates
├── src/
│   ├── lib.rs                       # re-exports all public types
│   ├── config.rs                    # OptimizationTarget, OptimizationConfig, CycleBudget, PromptSection
│   ├── diagnose.rs                  # Diagnoser, WeakPoint, TargetComponent, CaseFailure
│   ├── mutate.rs                    # MutationStrategy trait, MutationContext, MutationError, Candidate
│   ├── strategies/
│   │   ├── mod.rs                   # Strategy module re-exports
│   │   ├── llm_guided.rs           # LlmGuided strategy (uses JudgeClient)
│   │   ├── template_based.rs       # TemplateBased strategy + built-in template library
│   │   └── ablation.rs             # Ablation strategy (remove/simplify)
│   ├── evaluate.rs                  # CandidateEvaluator, MutatingAgentFactory, CandidateResult
│   ├── gate.rs                      # AcceptanceGate, AcceptanceResult, AcceptanceVerdict
│   ├── persist.rs                   # CyclePersister, ManifestEntry, output directory management
│   ├── runner.rs                    # EvolutionRunner (orchestrates the full cycle)
│   └── types.rs                     # BaselineSnapshot, CycleResult, CycleStatus
└── tests/
    ├── baseline.rs                  # US1: baseline evaluation
    ├── diagnose.rs                  # US2: weak point identification
    ├── mutate.rs                    # US3: candidate generation
    ├── gate.rs                      # US5: acceptance gating
    ├── persist.rs                   # US6: manifest writing
    └── end_to_end.rs               # US7: full cycle integration
```

**Structure Decision**: New `evolve/` directory at workspace root, following the pattern of `eval/`, `patterns/`, `policies/`. Strategies get their own subdirectory because each has distinct dependencies (JudgeClient for LLM-guided, regex for templates, string manipulation for ablation). One integration test file per user story.

## Notes

- **MutatingAgentFactory**: Wrapper around the caller's `AgentFactory` that intercepts `create_agent()` to inject the mutated system prompt or tool schemas. The inner factory handles all provider-specific agent construction. The wrapper only replaces the `system_prompt` field in `AgentOptions` or swaps tool schemas.
- **Section parsing**: `OptimizationTarget::new()` runs a regex scan for `^## ` headers (default) or the user's custom delimiter. Each match produces a `PromptSection` with name, content, and byte range. `with_replaced_section()` reconstructs the full prompt by replacing the byte range.
- **Template library**: Ships ~10 built-in transformation patterns. Each is a `(Regex, String)` pair. Templates are applied to the target text via `regex.replace_all()`. Deterministic when seeded — template order is fixed, but seed controls which subset is applied when `max_candidates_per_strategy` limits output.
- **LlmGuided prompt template**: Uses a structured prompt: "The following [section/tool description] scored {score} on evaluation case '{case_name}'. The expected behavior was: {criteria}. The failing trace was: {trace_summary}. Rewrite the text to improve the score. Return only the improved text." Sent via `JudgeClient::judge()`.
- **Budget propagation**: `CycleBudget` uses `Mutex<Cost>` for interior mutability. Passed as `Arc<CycleBudget>` to all phases. Each phase calls `budget.record(cost)` after its work and `budget.is_exhausted()` before starting expensive operations.
- **Manifest JSONL**: One line per candidate (accepted and rejected). Written atomically per cycle via `BufWriter` + `flush()`. The manifest is append-only — re-running a cycle appends, never overwrites.
- **Observability**: Each phase method is annotated with `#[cfg_attr(feature = "otel", tracing::instrument(skip_all, fields(cycle = %cycle_number)))]`. Span names: `evolve::baseline`, `evolve::diagnose`, `evolve::mutate`, `evolve::evaluate`, `evolve::gate`, `evolve::persist`.
- **Feature gates**: `otel` enables tracing spans (depends on `opentelemetry` workspace dep). Default features: none. `all` enables `otel`.
- **P1 case identification**: `AcceptanceGate` reads `case.metadata["priority"]` as a string. If the field is absent, null, or `"P1"`, the case is treated as P1. Only `"P2"` or `"P3"` (case-insensitive) exempt a case from regression protection.
