# Swink Agent — Evaluation Feature Roadmap

**Scope:** This document tracks the evaluation features in `swink-agent-eval`. Items marked `[C]` are complete, `[P]` are partially implemented, and `[ ]` are planned but not yet started.

**Legend:**
- `[C]` — Complete: fully implemented and tested
- `[P]` — Partial: core functionality exists with noted gaps
- `[ ]` — Planned: not yet implemented

---

## 1. Trajectory & Process Evaluation (The "Glass-Box" Layer)

[P] Fine-Grained Trajectory Tracing: Capture every step of the agent's execution, including tool calls and timing. `TrajectoryCollector` captures turns, tool calls (name/id/arguments), timing, cost, and usage per turn via `AgentEvent` stream observation. Gaps: no visualization component, no planning-node capture, no state-transition tracking beyond tool calls.

[C] "Golden Path" Comparison: Compare an agent's actual execution path against a predefined "ideal" sequence of tool calls and reasoning steps. Implemented via `TrajectoryMatcher`.

[C] Trajectory Efficiency Scoring: Metrics that penalize redundant tool calls and excessive step counts. `EfficiencyEvaluator` scores via duplicate ratio (unique/total tool calls, weight 0.6) and step ratio (ideal vs actual turn count, weight 0.4). Registered in `EvaluatorRegistry::with_defaults()`. Gaps: no reasoning-loop detection, no per-tool cost attribution.

[ ] Step-Level Reproducibility: A "replay" feature that allows developers to re-run simulations from any specific intermediate step to debug the exact moment of failure.

## 2. Advanced Verification & Judging

[ ] Agent-as-a-Judge (AaaJ) Support: Multi-agent "judge" systems that employ planning and tool-augmented evidence collection to verify outcomes rather than relying on a single-pass LLM.

[ ] Executable Verification (Sandboxing): Built-in sandboxed environments where a judge can execute the agent's proposed code or API calls to verify side effects and factual correctness.

[P] Domain-Expert Outcome Scoring: User-provided scoring functions for custom success criteria. `ResponseCriteria::Custom` accepts an `Arc<dyn Fn(&str) -> Score>`, and the `Evaluator` trait supports closure-based evaluators. Gap: requires Rust code — no plain-language or declarative interface for non-technical users.

[ ] Automated Root Cause Analysis: AI-driven insights that automatically group failures into patterns (e.g., "tool hallucination," "instruction drift") to prioritize fixes.

## 3. Simulation & Stress Testing

[ ] Dynamic Environment Simulation: Support for "asynchronous" testing where the environment state can change independently of the agent's actions.

[ ] Synthetic Persona Generation: Simulate diverse user personas and adversarial edge cases (e.g., ambiguous queries, prompt injections).

[ ] Temporal Reasoning Verification: Test if agents can adhere to strict time constraints.

## 4. Observability & Standards

[ ] OpenInference/OTEL Compliance: Native support for OpenInference semantic conventions, ensuring traces are portable across any OpenTelemetry-compatible backend.

[C] Cost-Latency Spiral Monitoring: Real-time tracking of token usage and financial cost per resolution. `BudgetGuard` monitors cost, token, and turn thresholds in real-time during stream collection via `TrajectoryCollector::collect_with_guard()`. Exceeding any threshold triggers `CancellationToken` cancellation. `EvalRunner::run_case()` automatically wires `BudgetGuard` from `EvalCase` budget constraints. `BudgetEvaluator` provides post-hoc scoring. Gap: no per-tool cost attribution.

[ ] Distributed Multi-Agent Tracing: Stitch together execution traces across multiple agents and sub-agents into a unified "Agent Graph".

## 5. Production Readiness & Governance

[ ] Real-Time Guardrail Conversion: Automatically convert successful evaluation criteria into real-time production guardrails.

[C] CI/CD Gating: `GateConfig` checks evaluation results against pass-rate, cost, and duration thresholds. Can be wired into deployment pipelines to block releases when benchmarks regress.

[ ] Human-in-the-Loop (HITL) Annotation Queues: Structured workflows for subject matter experts to review and label "uncertain" traces, feeding back into the evaluation dataset.

[C] Deterministic Audit Trails: `AuditedInvocation` wraps invocation traces with SHA-256 hash chains for tamper detection. Each turn is hashed individually; concatenated hashes produce a chain hash verified via `AuditedInvocation::verify()`.
