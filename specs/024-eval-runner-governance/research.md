# Research: Eval Runner, Scoring & Governance

**Feature**: 024-eval-runner-governance
**Date**: 2026-03-23

## Status

All research items resolved — no NEEDS CLARIFICATION items remain. The feature is fully implemented; research confirms alignment between spec and implementation.

## Research Items

### 1. Eval Case Data Format

**Decision**: JSON as primary format; YAML as opt-in via `yaml` feature gate.
**Rationale**: JSON is zero-dependency (already in workspace via `serde_json`). YAML is more human-friendly for authoring but adds an optional dependency (`serde_yaml`). Feature-gating keeps the default dependency footprint minimal.
**Alternatives Considered**:
- TOML: Poor fit for deeply nested structures (eval cases have nested tool call arguments, response criteria).
- RON (Rusty Object Notation): Niche, low adoption outside Rust gamedev.

### 2. Hash Algorithm for Audit Trails

**Decision**: SHA-256 via the `sha2` crate.
**Rationale**: Industry standard, widely audited, sufficient collision resistance for integrity verification. The `sha2` crate is pure Rust, no C FFI, aligns with `#[forbid(unsafe_code)]`.
**Alternatives Considered**:
- BLAKE3: Faster but less universally recognized for compliance contexts.
- SHA-3 (Keccak): Overkill; SHA-256 is the de facto standard.
- HMAC: Not needed — we're proving integrity, not authenticity. The threat model is post-hoc tampering detection, not origin verification.

### 3. Score Aggregation Strategy

**Decision**: Per-case verdict is all-must-pass (AND logic). Per-set summary counts passed/failed cases and aggregates cost/usage/duration.
**Rationale**: AND logic is the strictest interpretation — a case passes only if every applicable evaluator passes. This is appropriate for CI/CD gating where false positives are costly. The set summary provides pass rate for threshold-based gating.
**Alternatives Considered**:
- Weighted average: More nuanced but harder to reason about in gate thresholds.
- Any-pass (OR logic): Too lenient for CI/CD safety gates.

### 4. Persistence Layout

**Decision**: `{dir}/sets/{id}.json` for eval set definitions; `{dir}/results/{eval_set_id}/{timestamp}.json` for run results.
**Rationale**: Timestamp-keyed results enable natural chronological ordering. Separating sets from results keeps definitions stable while results accumulate. JSON with `to_string_pretty` ensures human-readability.
**Alternatives Considered**:
- Single file per set (appending results): Risk of corruption, harder to list/load individual runs.
- SQLite: Heavier dependency, violates library-first principle for a persistence concern that's sufficient with flat files.

### 5. Budget Enforcement Strategy

**Decision**: Two-layer approach — `BudgetGuard` for real-time enforcement (abort via `CancellationToken`), `BudgetEvaluator` for post-hoc scoring.
**Rationale**: Real-time enforcement prevents runaway cost during eval. Post-hoc scoring enables trend analysis and threshold-based alerting. The two layers serve different concerns and compose naturally.
**Alternatives Considered**:
- Post-hoc only: Allows runaway cost before detection.
- Real-time only: No historical budget scoring or trend analysis.

### 6. Evaluator Composition Pattern

**Decision**: Registry pattern with `Vec<Arc<dyn Evaluator>>`. Optional name-based filtering per case via `EvalCase::evaluators`.
**Rationale**: Registry enables default + custom evaluator composition. Name filtering allows cases to opt into specific evaluators without affecting the global registry. `Arc<dyn Evaluator>` allows shared ownership across concurrent usage.
**Alternatives Considered**:
- Trait object chain (decorator pattern): More complex, harder to inspect/debug.
- Compile-time generics: Prevents runtime registration of custom evaluators.

### 7. Runner Error Handling for Case Failures

**Decision**: Agent errors during case execution propagate as `EvalError::Agent`; the current implementation returns the error from `run_set` rather than recording it and continuing.
**Rationale**: The spec says "continue executing remaining cases when one case fails" (FR-003). The acceptance scenario says "the failure is recorded for that case and the suite continues." The current `run_set` returns `Err(e)` on agent error (runner.rs:127-129), which stops the suite. **This is a gap** — the implementation should catch agent errors per-case and record them as failed results rather than aborting the suite. However, evaluator failures within a case are handled correctly (they return `None` and other evaluators still run). This gap should be addressed in task generation.
