1. Trajectory & Process Evaluation (The "Glass-Box" Layer)
[P] Fine-Grained Trajectory Tracing: Ability to capture and visualize every step of the agent's execution, including planning nodes, tool calls, and state transitions. ANALYSIS NOTE: TrajectoryCollector captures turns, tool calls (name/id/arguments), timing, cost, and usage per turn via AgentEvent stream observation. However, there is no visualization component, no planning-node capture, and no state-transition tracking beyond tool calls.

[C] "Golden Path" Comparison: Capability to compare an agent's actual execution path against a predefined "ideal" sequence of tool calls and reasoning steps.

[C] Trajectory Efficiency Scoring: Metrics that penalize redundant tool calls, unnecessary reasoning loops, and excessive step counts to optimize for cost and latency. ANALYSIS NOTE: EfficiencyEvaluator scores trajectory waste via two sub-metrics: duplicate ratio (unique/total tool calls, weight 0.6) and step ratio (ideal vs actual turn count, weight 0.4). Registered in EvaluatorRegistry::with_defaults(). Remaining gaps: no reasoning-loop detection and no per-tool cost attribution.

[ ] Step-Level Reproducibility: A "replay" feature that allows developers to re-run simulations from any specific intermediate step to debug the exact moment of failure.

2. Advanced Verification & Judging
[ ] Agent-as-a-Judge (AaaJ) Support: Utilization of multi-agent "judge" systems that employ planning and tool-augmented evidence collection to verify outcomes rather than relying on a single-pass LLM.

[ ] Executable Verification (Sandboxing): Built-in sandboxed environments where a judge can actually execute the agent's proposed code or API calls to verify side effects and factual correctness.

[W] Domain-Expert "Outcome Scoring": No-code interfaces that allow non-technical domain experts (e.g., legal or medical professionals) to define success criteria in plain language. ANALYSIS NOTE: ResponseCriteria::Custom accepts an Arc<dyn Fn(&str) -> Score> allowing user-provided scoring functions, and the Evaluator trait supports closure-based evaluators via blanket impl on tuples. However, this requires Rust code — there is no plain-language, no-code, or declarative interface for non-technical users.

[ ] Automated Root Cause Analysis: AI-driven insights that automatically group failures into patterns (e.g., "tool hallucination," "instruction drift") to prioritize fixes.

3. Simulation & Stress Testing
[ ] Dynamic Environment Simulation: Support for "asynchronous" testing where the environment state (e.g., a calendar or database) can change independently of the agent's actions.

[ ] Synthetic Persona Generation: The ability to simulate hundreds of diverse user personas and "adversarial" edge cases (e.g., ambiguous queries, frustrated users, or prompt injections).

[ ] Temporal Reasoning Verification: Capability to test if agents can adhere to strict time constraints (e.g., "Schedule this exactly at 2:00 PM").

4. Observability & Standards
[ ] OpenInference/OTEL Compliance: Native support for OpenInference semantic conventions, ensuring traces are portable across any OpenTelemetry-compatible backend.

[C] Cost-Latency Spiral Monitoring: Real-time tracking of token usage and financial cost per resolution to identify "runaway" agents before they drain budgets. ANALYSIS NOTE: BudgetGuard monitors cost, token, and turn thresholds in real-time during stream collection via TrajectoryCollector::collect_with_guard(). Exceeding any threshold triggers CancellationToken cancellation with a tracing warning. EvalRunner::run_case() automatically wires BudgetGuard from EvalCase budget constraints. BudgetEvaluator still provides post-hoc scoring. Remaining gap: no per-tool cost attribution.

[ ] Distributed Multi-Agent Tracing: Ability to stitch together execution traces across multiple agents and sub-agents into a unified "Agent Graph".

5. Production Readiness & Governance
[ ] Real-Time Guardrail Conversion: Feature to automatically convert successful evaluation criteria into real-time production guardrails.

[ ] CI/CD Gating: Integration with deployment pipelines (e.g., GitHub Actions) to block releases if an agent's success rate on core benchmarks drops.

[ ] Human-in-the-Loop (HITL) Annotation Queues: Structured workflows for subject matter experts to review and label "uncertain" traces, which then feed back into the evaluation dataset.

[ ] Deterministic Audit Trails: Cryptographically signed logs and "decision receipts" to ensure every action taken by an autonomous system is explainable and auditable for compliance.
