# Implementation Plan: Eval Runner, Scoring & Governance

**Branch**: `024-eval-runner-governance` | **Date**: 2026-03-23 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/024-eval-runner-governance/spec.md`

## Summary

Evaluation orchestration pipeline within the `swink-agent-eval` crate. Provides `EvalRunner` for executing evaluation suites, `Evaluator` trait + `EvaluatorRegistry` for composable scoring, `Score`/`Verdict` types, `GateConfig` for CI/CD deployment gating, `EvalStore`/`FsEvalStore` for filesystem persistence, `AuditedInvocation` for SHA-256 hash chain tamper detection, and `BudgetEvaluator`/`EfficiencyEvaluator` for resource governance. All functionality lives in the existing `eval/` crate — no new crates required.

## Technical Context

**Language/Version**: Rust 1.88 (edition 2024)
**Primary Dependencies**: `swink-agent` (core), `serde`/`serde_json`, `tokio`/`tokio-util`, `futures`, `sha2`, `regex`, `thiserror`, `tracing`, `uuid`; optional `serde_yaml` via `yaml` feature gate
**Storage**: Local filesystem via JSON files (`FsEvalStore`); optional YAML input via feature gate
**Testing**: `cargo test -p swink-agent-eval` + `tempfile`, `pretty_assertions`
**Target Platform**: Cross-platform (macOS, Linux, Windows)
**Project Type**: Library crate (workspace member of `swink-agent`)
**Performance Goals**: N/A — eval runs are inherently I/O-bound (LLM calls); sequential case execution by design
**Constraints**: Sequential case execution; filesystem-only persistence; no unsafe code
**Scale/Scope**: Suites of 1–100+ eval cases; results persisted as individual JSON files per run

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | ✅ Pass | All functionality in `eval/` crate; no service, no daemon; independently compilable and testable |
| II. Test-Driven Development | ✅ Pass | 11 test files (1444 lines); tests for every module; shared helpers in `tests/common/mod.rs`; mocks prefixed `Mock` |
| III. Efficiency & Performance | ✅ Pass | No hot-path allocations; sequential execution is appropriate for eval workloads; budget guard uses `CancellationToken` for efficient abort |
| IV. Leverage the Ecosystem | ✅ Pass | Uses `sha2` (not custom hash), `serde_yaml` (not custom parser), `regex`, `thiserror`; all deps centralized in workspace `Cargo.toml` |
| V. Provider Agnosticism | ✅ Pass | `AgentFactory` trait decouples runner from agent construction; no provider-specific types in eval crate |
| VI. Safety & Correctness | ✅ Pass | `#[forbid(unsafe_code)]` at crate root; clippy all/pedantic/nursery as errors; errors produce structured `EvalError`, never panics |

**Architectural Constraints**:
- Crate count: ✅ No new crate — uses existing `eval/` (7th workspace member)
- MSRV: ✅ 1.88, edition 2024
- Concurrency: ✅ Sequential case execution; `BudgetGuard` uses `CancellationToken` for abort
- Events: ✅ `TrajectoryCollector` observes events outward-only via `AgentEvent` stream
- No global mutable state: ✅ All shared state in `Arc`; no global mutables

## Project Structure

### Documentation (this feature)

```text
specs/024-eval-runner-governance/
├── plan.md              # This file
├── spec.md              # Feature specification
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── evaluator-trait.md
└── tasks.md             # Phase 2 output (NOT created by /speckit.plan)
```

### Source Code (repository root)

```text
eval/
├── Cargo.toml
├── CLAUDE.md
├── src/
│   ├── lib.rs           # Public API re-exports
│   ├── types.rs         # Core data types: EvalCase, EvalSet, Invocation, TurnRecord, results
│   ├── evaluator.rs     # Evaluator trait + EvaluatorRegistry
│   ├── score.rs         # Score (0.0-1.0) + Verdict (Pass/Fail)
│   ├── runner.rs        # EvalRunner + AgentFactory trait
│   ├── trajectory.rs    # TrajectoryCollector + BudgetGuard
│   ├── match_.rs        # TrajectoryMatcher (Exact/InOrder/AnyOrder)
│   ├── response.rs      # ResponseMatcher (Exact/Contains/Regex/Custom)
│   ├── budget.rs        # BudgetEvaluator (cost/tokens/turns/duration)
│   ├── efficiency.rs    # EfficiencyEvaluator (duplicate + step ratio)
│   ├── gate.rs          # GateConfig + check_gate() + GateResult
│   ├── store.rs         # EvalStore trait + FsEvalStore (JSON persistence)
│   ├── audit.rs         # AuditedInvocation (SHA-256 hash chains)
│   ├── error.rs         # EvalError enum
│   └── yaml.rs          # YAML loading (feature-gated)
└── tests/
    ├── common/mod.rs    # Shared test helpers
    ├── audit.rs         # Hash chain tests
    ├── budget.rs        # Budget evaluator tests
    ├── efficiency.rs    # Efficiency evaluator tests
    ├── gate.rs          # Gate threshold tests
    ├── match_.rs        # Trajectory matching tests
    ├── response.rs      # Response matching tests
    ├── score.rs         # Score/Verdict tests
    ├── store.rs         # FsEvalStore persistence tests
    ├── trajectory.rs    # TrajectoryCollector tests
    └── yaml.rs          # YAML loading tests
```

**Structure Decision**: Single existing crate (`eval/`). One concern per file, 15 source modules, 11 test files. No structural changes needed — the crate boundary already owns the eval concern per constitution.
