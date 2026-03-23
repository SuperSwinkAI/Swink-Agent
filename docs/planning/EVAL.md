> **Archived** — All phases complete as of 2026-03-14. Kept for historical reference.

---

# Swink Agent — Evaluation Feature Roadmap

**Scope:** This document tracks the evaluation features in `swink-agent-eval`. Items marked `[C]` are complete, `[P]` are partially implemented, and `[ ]` are planned but not yet started.

**Legend:**
- `[C]` — Complete: fully implemented and tested
- `[P]` — Partial: core functionality exists with noted gaps
- `[ ]` — Planned: not yet implemented

---

## Implemented Features

The following features are complete and tested in `swink-agent-eval`.

### Trajectory & Process Evaluation

[C] **Fine-Grained Trajectory Tracing:** `TrajectoryCollector` captures turns, tool calls (name/id/arguments), timing, cost, and usage per turn via `AgentEvent` stream observation. Also supports real-time budget guarding via `collect_with_guard()`. Known gaps: no visualization component, no planning-node capture, no state-transition tracking beyond tool calls.

[C] **"Golden Path" Comparison:** `TrajectoryMatcher` compares an agent's actual execution path against a predefined ideal sequence of tool calls (Exact / InOrder / AnyOrder modes).

[C] **Trajectory Efficiency Scoring:** `EfficiencyEvaluator` scores via duplicate ratio (weight 0.6) and step ratio (weight 0.4). Registered in `EvaluatorRegistry::with_defaults()`. Known gaps: no reasoning-loop detection, no per-tool cost attribution.

### Observability & Governance

[C] **Cost-Latency Spiral Monitoring:** `BudgetGuard` monitors cost, token, and turn thresholds in real-time during stream collection via `TrajectoryCollector::collect_with_guard()`. Exceeding any threshold triggers `CancellationToken` cancellation. `EvalRunner::run_case()` automatically wires `BudgetGuard` from `EvalCase` budget constraints. `BudgetEvaluator` provides post-hoc scoring. Known gap: no per-tool cost attribution.

[C] **CI/CD Gating:** `GateConfig` checks evaluation results against pass-rate, cost, and duration thresholds via `check_gate()`. Returns `GateResult` with exit code 0 (pass) or 1 (fail). Can be wired into deployment pipelines to block releases when benchmarks regress.

[C] **Deterministic Audit Trails:** `AuditedInvocation` wraps invocation traces with SHA-256 hash chains for tamper detection. Each turn is hashed individually; concatenated hashes produce a chain hash verified via `AuditedInvocation::verify()`.

### Verification

[P] **Domain-Expert Outcome Scoring:** `ResponseCriteria::Custom` accepts an `Arc<dyn Fn(&str) -> Score>`, and the `Evaluator` trait supports closure-based evaluators. Gap: requires Rust code — no plain-language or declarative interface for non-technical users.

### Data Loading

[C] **YAML Eval Specs:** `load_eval_set_yaml()` loads eval sets from YAML files (requires `yaml` feature gate). All `ResponseCriteria` variants except `Custom` are supported.

---

## Planned / Future Features

The following features are not yet implemented.

### Trajectory & Process Evaluation

[ ] **Step-Level Reproducibility:** A "replay" feature that allows developers to re-run simulations from any specific intermediate step to debug the exact moment of failure.

### Advanced Verification & Judging

[ ] **Agent-as-a-Judge (AaaJ) Support:** Multi-agent "judge" systems that employ planning and tool-augmented evidence collection to verify outcomes.

[ ] **Executable Verification (Sandboxing):** Built-in sandboxed environments where a judge can execute the agent's proposed code or API calls.

[ ] **Automated Root Cause Analysis:** AI-driven insights that automatically group failures into patterns (e.g., "tool hallucination," "instruction drift").

### Simulation & Stress Testing

[ ] **Dynamic Environment Simulation:** Support for "asynchronous" testing where the environment state can change independently of the agent's actions.

[ ] **Synthetic Persona Generation:** Simulate diverse user personas and adversarial edge cases.

[ ] **Temporal Reasoning Verification:** Test if agents can adhere to strict time constraints.

### Observability & Standards

[ ] **OpenInference/OTEL Compliance:** Native support for OpenInference semantic conventions for portable traces.

[ ] **Distributed Multi-Agent Tracing:** Stitch together execution traces across multiple agents and sub-agents into a unified "Agent Graph".

### Production Readiness & Governance

[ ] **Real-Time Guardrail Conversion:** Automatically convert successful evaluation criteria into real-time production guardrails.

[ ] **Human-in-the-Loop (HITL) Annotation Queues:** Structured workflows for subject matter experts to review and label "uncertain" traces.
