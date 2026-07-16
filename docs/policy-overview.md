# Swink Agent: Policy Guardrails

AI agents make decisions on their own. They reason, call tools, and produce responses without waiting for a human to approve every step. That speed is what makes them useful, but it also means you need built-in controls to keep things safe.

Swink Agent handles this with **policy guardrails**, a series of configurable checkpoints embedded directly in the agent loop. Every LLM call, every tool execution, and every response passes through these gates. They enforce safety, cost, and compliance rules *before* anything reaches the outside world.

Because these controls are part of the framework itself, every agent built as a Swink Agent inherits them automatically. Developers don't bolt on safety after the fact. Policies evaluate at the instance level, run entirely on-device, and work with both cloud and local models. No external service required. They add negligible latency since they run as native compiled code, not additional LLM calls.

---

## How It Works

The agent loop is the engine. Policy checkpoints wrap each stage so nothing passes through unchecked.

![Policy Guardrails Diagram](policy-guardrails.svg)

Each guard can **approve**, **modify**, or **halt** the agent before the next stage runs.

| Color | Checkpoint | When It Runs |
|:-----:|-----------|-------------|
| <span style="color:#2563eb">&#9632;</span> Blue | Pre-Turn | Before the AI begins reasoning |
| <span style="color:#d97706">&#9632;</span> Amber | Pre-Dispatch | Before a tool is allowed to execute |
| <span style="color:#16a34a">&#9632;</span> Green | Post-Turn | After the AI responds, before delivery |
| <span style="color:#94a3b8">&#9632;</span> Grey | Post-Loop | After the full loop completes |

---

## Example Policies

| Policy | Checkpoint | What It Does |
|--------|-----------|-------------|
| **Budget** | Pre-Turn | Stops the agent when cost or token usage exceeds a set limit |
| **Max Turns** | Pre-Turn | Caps the number of reasoning cycles the agent can take |
| **Tool Deny List** | Pre-Dispatch | Blocks specific tools from being used (e.g., file write, shell access) |
| **Sandbox** | Pre-Dispatch | Restricts file access to an approved directory and rejects path traversal |
| **Loop Detection** | Post-Turn | Detects when the agent is stuck repeating the same action |
| **Checkpoint** | Post-Turn | Saves agent state after each turn for recovery and audit |
| **Prompt Injection Guard** | Pre-Turn + Post-Turn | Detects attempts to override the agent's instructions |
| **PII Redactor** | Post-Turn | Removes personally identifiable information from responses |
| **Content Filter** | Post-Turn | Blocks responses containing prohibited keywords or patterns |
| **Audit Logger** | Post-Turn | Records every turn to a persistent log for compliance review |
| **Memory Nudge** | Post-Turn | Heuristically detects save-worthy content in agent turns and nudges it toward memory |

---

## Recommended Production Policy Set

The library default is **anything-goes**: with no policies configured, the agent loop runs with no restrictions (`specs/031-policy-slots/spec.md` FR-009). That default is correct for the library — it's the right posture for experimentation, tests, and callers who want to compose their own guardrails from scratch.

It is **not** a safe default for an autonomous agent running unattended in production. Embedders (for example, a daemon that runs agents on a schedule or in response to external events) MUST wire at least these four policies themselves:

| Policy | Checkpoint | Crate path |
|--------|-----------|------------|
| **BudgetPolicy** | Pre-Turn | `policies/src/budget.rs` (`swink-agent-policies`, feature `budget`) |
| **MaxTurnsPolicy** | Pre-Turn | `policies/src/max_turns.rs` (feature `max-turns`) |
| **SandboxPolicy** | Pre-Dispatch | `policies/src/sandbox.rs` (feature `sandbox`) |
| **ToolDenyListPolicy** | Pre-Dispatch | `policies/src/deny_list.rs` (feature `deny-list`) |

Each can be wired individually via `AgentOptions::with_pre_turn_policy(...)` / `with_pre_dispatch_policy(...)` — see `policies/examples/with_policies.rs` and `policies/README.md` for the full chain — or all four at once via the bundled `RecommendedPolicies` builder ([#1065](https://github.com/SuperSwinkAI/Swink-Agent/issues/1065)), which also ships an integration-contract test embedders can run to verify their own wiring didn't accidentally drop a guardrail (see below).

---

## Recommended Production Policy Set

The library ships anything-goes: no policies run unless enabled. Deployments that run agents autonomously should wire, at minimum, **Budget**, **Max Turns**, **Sandbox**, and **Tool Deny List**. The `swink-agent-policies` crate bundles exactly this set behind the `recommended` feature:

```rust,ignore
use swink_agent_policies::RecommendedPolicies;

let options = RecommendedPolicies::builder()
    .with_max_cost(10.0)                       // stop at $10 total spend (default)
    .with_max_turns(50)                        // stop after 50 turns (default)
    .with_sandbox_root("/srv/agent-workspace") // restrict file tools (default: cwd)
    .with_deny_tools(["bash"])                 // block shell access (default)
    .apply(options);
```

Pair it with the contract-test helper `assert_production_guardrails(&options, "bash")` in your test suite: it verifies all four policies are present and configured with non-trivial limits, so a refactor can't silently drop a guardrail. See the [`swink-agent-policies` README](../policies/README.md#recommended-production-guardrails) for details.

---

## Key Properties

- **Default-off.** No policies run unless explicitly enabled. Zero overhead when unused.
- **Composable.** Multiple policies can run at the same checkpoint. They evaluate in order, and if any policy says stop, the agent stops.
- **Isolated.** A failing policy cannot crash the agent. Panics are caught and the policy is automatically removed.
- **Extensible.** Custom policies can be built against the public trait API without modifying the core.
