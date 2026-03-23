---
name: qa
description: Run a comprehensive multi-agent QA audit of the entire project. Checks API/component boundaries, DRY violations, dead code, AGENTS.md style guide compliance, linting, type checking, tests, documentation accuracy, and extensibility of the core agent harness.
disable-model-invocation: true
allowed-tools: Agent, Bash, Read, Grep, Glob
argument-hint: [optional: crate or focus area]
---

# Multi-Agent QA Audit

Run a comprehensive quality audit of the Swink-Agent workspace. Launch all agents in parallel for maximum speed.

**Project mission context:** This project is a robust "core" agent harness designed to support experimentation around agent memory, tool use, agent teams, and long-running patterns. Every audit dimension should be evaluated through this lens — the core must be modular, extensible, and plugin-friendly.

## Scope

If $ARGUMENTS is provided, narrow the audit to that crate or area. Otherwise audit the full workspace.

## Step 1 — Build, Lint & Test (run first, blocking)

Run these three commands in parallel using the Bash tool. If any fail, report the failures but continue with the remaining audit steps.

```bash
cargo clippy --workspace -- -D warnings
cargo test --workspace
cargo test -p swink-agent --no-default-features
```

## Step 2 — Launch parallel audit agents

After Step 1 completes, launch ALL of the following agents concurrently using the Agent tool. Each agent should report findings as a structured list of issues with file paths and line numbers. **If an agent finds no issues in its area, return a single line: "No findings." — do not elaborate.**

### Agent 1: API & Component Boundaries

subagent_type: Explore

Audit the public API surface and component boundaries across all crates in the workspace:

1. Read every `lib.rs` to understand what each crate re-exports.
2. Check that consumers never need to reach into submodules — `lib.rs` should re-export the full public API.
3. Identify any leaky abstractions: public types that expose internal implementation details, or crate-internal types that are accidentally `pub`.
4. Check that crate boundaries are clean — no circular dependencies, no crate reaching into another's internals.
5. Verify `new()` primary constructors and `with_*()` builder chains are used consistently.
6. Check error types use named constructors (e.g., `AgentError::network(err)`).
7. Flag any `get_` prefixed getters (should be bare name) or predicates missing `is_`/`has_` prefix.

Report each issue with: file path, line number, what's wrong, and suggested fix.

### Agent 2: DRY & Dead Code

subagent_type: Explore

Search the entire workspace for DRY violations and dead code:

1. Look for duplicated logic, repeated patterns, or copy-pasted code blocks across files and crates.
2. Identify unused functions, structs, enums, traits, constants, and imports. Pay attention to `#[allow(dead_code)]` annotations — flag any that may be masking genuinely unused code.
3. Check for unused feature flags or cfg attributes that gate code no longer needed.
4. Look for TODO/FIXME/HACK comments referencing completed work that can be cleaned up.
5. Check for test helpers or utilities defined in multiple places that should be consolidated.

Report each issue with: file path, line number, what's duplicated or dead, and suggested action.

### Agent 3: Style Guide Compliance

subagent_type: Explore

Read the root AGENTS.md and every nested AGENTS.md in the workspace. Then audit the codebase for violations:

1. **Module naming:** Trailing `_` for reserved-word modules (e.g., `loop_.rs`).
2. **Closure aliases:** Must be suffixed with `Fn` (e.g., `ConvertToLlmFn`).
3. **Constructors:** `new()` primary, `with_*()` builder chain. Named constructors on error types.
4. **Getters:** No `get_` prefix. `is_*`/`has_*` for predicates.
5. **Import order:** `std` → external (alphabetical) → `crate::`/`super::`.
6. **Test names:** Descriptive `snake_case` without `test_` prefix. Mocks prefixed `Mock`.
7. **File size:** Flag any file exceeding ~1500 lines — should be split.
8. **One concern per file:** Flag files that mix unrelated responsibilities.
9. **`#[forbid(unsafe_code)]`:** Must be at every crate root.
10. **Shared test helpers:** Should be in `tests/common/mod.rs`, not scattered.

Report each violation with: rule violated, file path, line number, and what to fix.

### Agent 4: Extensibility & Core Harness Fitness

subagent_type: Explore

This project's goal is to be a robust core for experimentation around agent memory, tool use, agent teams, and long-running patterns. Audit how well the current implementation serves as an extensible harness that plugins, wrappers, and experimental crates can build on top of.

**Trait extensibility:**
1. Read every public trait in the workspace. For each, assess: Can an external crate implement this trait to extend behavior? Are trait bounds overly restrictive (e.g., requiring concrete types instead of trait objects)?
2. Identify traits that are missing but should exist — places where behavior is hardcoded that should be pluggable (e.g., memory backends, message routing, agent orchestration strategies).
3. Check that traits use associated types or generics appropriately to avoid boxing overhead where performance matters.

**Plugin / wrapper friendliness:**
4. Can an external crate wrap `Agent` to add behavior (middleware pattern)? Identify any `pub(crate)` or private fields/methods that prevent this.
5. Are lifecycle events (via `AgentEventSubscriber` or similar) sufficient for an external observer to track and react to everything meaningful? Identify missing event hooks.
6. Is the tool system open enough for external tool implementations? Check `AgentTool` trait bounds, registration, and schema generation.
7. Can context/message types be extended (e.g., custom message types, metadata) without forking the core?

**Memory & state extensibility:**
8. Is there a clean abstraction for agent memory/state persistence? Or is memory handling baked into concrete types?
9. Can conversation history be externally managed (injected, compacted, migrated)?

**Multi-agent / team patterns:**
10. Can multiple `Agent` instances coordinate? Is there an orchestration layer, or must consumers build their own?
11. Are message types and tool results shareable across agents?
12. Is there support (or clean extension points) for agent-to-agent communication?

**Long-running patterns:**
13. Is there support for checkpointing, resuming, or serializing agent state mid-conversation?
14. Can the loop be paused, inspected, and resumed externally?
15. Are there clean shutdown / cancellation semantics?

**Produce a summary table** with these columns:

| Area | Current State | Gap / Opportunity | Impact | Effort |
|------|--------------|-------------------|--------|--------|

Where:
- **Area**: The extensibility dimension (e.g., "Memory backends", "Agent teams", "Tool registration")
- **Current State**: Brief description of what exists today
- **Gap / Opportunity**: What's missing or could be improved for plugin/wrapper developers
- **Impact**: High / Medium / Low — how much this matters for the core harness mission
- **Effort**: Small / Medium / Large — estimated implementation effort

Order the table by Impact (High first), then by Effort (Small first within same impact).

### Agents 5–12: Deep Crate Integration & Consumer Experience Analysis (8-agent team)

Launch up to 8 agents concurrently for this section. Each agent focuses on a specific crate boundary or cross-cutting concern. These agents should take their time — move slowly and deliberately, double-check each component, and re-read code a second time when uncertain. Long-term viability of the project is the key goal; thoroughness matters more than speed.

**Agent 5: Core crate internal coherence** (subagent_type: Explore)

Read every file in `src/`. Map the internal dependency graph between modules (agent.rs, loop_.rs, stream.rs, context.rs, tool.rs, etc.). For each module boundary:
- Is the separation of concerns clean? Could a module be understood in isolation?
- Are there circular references or tightly coupled modules that should be decoupled?
- Are there responsibilities that have drifted into the wrong module over time?
Take your time. Read each file fully, then re-examine the connections between them. Report any concern with file:line references.

**Agent 6: Core ↔ Adapters boundary** (subagent_type: Explore)

Read `src/lib.rs` and `adapters/src/lib.rs` fully. Then read every file in both crates. Analyze the boundary:
- What types/traits does the core expose that adapters depend on? Are these stable, minimal, and well-defined?
- Does the adapters crate duplicate any logic that belongs in core?
- Could a third-party adapter be written without depending on adapters (only on core)? If not, what's blocking that?
- Are streaming abstractions (`StreamFn`, `ProxyStreamFn`) cleanly separated between the two crates?
Double-check your findings by re-reading the relevant trait definitions and their implementations.

**Agent 7: Core ↔ Local-LLM boundary** (subagent_type: Explore)

Read `src/lib.rs` and `local-llm/src/lib.rs` fully. Then read every file in the local-llm crate. Analyze:
- Does local-llm cleanly implement core traits, or does it work around them?
- Are there types or abstractions that local-llm needs from core but can't access cleanly?
- Is model management (loading, configuration, presets) properly separated from the agent loop?
- Could local-llm be swapped out entirely without touching core? What coupling exists?
Move deliberately — check each public type in local-llm and trace how it connects to core.

**Agent 8: Core ↔ Eval boundary** (subagent_type: Explore)

Read `src/lib.rs` and `eval/src/lib.rs` fully. Then read every file in the eval crate. Analyze:
- Does eval cleanly consume the core's public API, or does it reach into internals?
- Are eval's gate/budget/audit abstractions general enough for diverse experiment types?
- Could an external crate build custom eval harnesses using only the public API?
- Is there test infrastructure in eval that duplicates what's in `tests/common/`?
Re-read the gate system implementation twice to ensure your analysis is accurate.

**Agent 9: Core ↔ TUI boundary** (subagent_type: Explore)

Read `src/lib.rs` and `tui/src/app/agent_bridge.rs` fully. Then read the rest of the TUI crate. Analyze:
- Is the agent bridge a clean consumer of the core API, or does it use workarounds?
- Does the TUI depend on any types that shouldn't be public?
- Could a different frontend (web, CLI, GUI) be built with the same ease? What's TUI-specific that should be generic?
- Are event subscriptions and streaming cleanly consumed?
Double-check by tracing a complete message flow from user input through the bridge to the agent and back.

**Agent 10: Cross-crate DRY analysis** (subagent_type: Explore)

This agent focuses exclusively on code duplication ACROSS crate boundaries (Agent 2 covers within-crate):
- Compare type definitions, error types, utility functions, and trait implementations across all crates.
- Identify patterns that are reimplemented in multiple crates and should be consolidated into core or a shared utilities module.
- Check for configuration/preset patterns duplicated between adapters, local-llm, and core.
- Look for conversion boilerplate (From/Into impls) that suggests the type boundaries are in the wrong place.
Read each crate's lib.rs and types side-by-side. Do not rush — systematic comparison is the goal.

**Agent 11: Consumer experience via custom_agent.rs** (subagent_type: Explore)

Read `examples/custom_agent.rs` very carefully — line by line. This is the primary example consumers see. Evaluate with extreme attention to clarity:
- Is the example simple and approachable? Could a developer new to the project understand it in one read?
- Are there unnecessary imports, boilerplate, or ceremony that could be eliminated?
- Does the example demonstrate the right abstractions? Are consumers forced to understand internals they shouldn't need to know about?
- Are builder patterns, constructors, and configuration intuitive?
- Count the number of distinct types/traits a consumer must understand to build a basic agent. Is this number appropriate?
- Compare the example against what the ideal consumer experience would look like. Draft what a "dream API" usage would look like and identify the gap.
Then read any other files in `examples/` for comparison. Report specific simplification opportunities with before/after sketches where helpful.

**Agent 12: Long-term viability & architectural risks** (subagent_type: Explore)

Step back and assess the project holistically for long-term viability as an experimental core:
- Read Cargo.toml (root and all crates) — are dependency choices appropriate and minimal? Any heavy deps that could be optional?
- Is the feature flag strategy sustainable? Could it grow cleanly as new experimental crates are added?
- Are there architectural decisions baked in now that would be painful to change later (e.g., specific async runtime coupling, serialization format choices, message type hierarchies)?
- Is the crate topology correct? Should any crates be merged or split further?
- Evaluate the error type hierarchy across the workspace — is it consistent and extensible?
Move slowly. Read the root Cargo.toml, then each crate's Cargo.toml, then summarize the dependency graph and flag concerns.

---

**Each of Agents 5–12 must produce:**
1. A findings list with file:line references. Omit any area that is all clear — one line "All clear: [area]" is sufficient. Do not elaborate on clean areas.
2. If there are actionable improvements: specific recommendations with file:line references. If there are none, say "No actionable improvements." — nothing more.

### Agents 13–22: Documentation Quality & Accuracy (10-agent team)

Launch up to 10 agents concurrently. The goal is to verify that every doc reflects the current implementation and is in an excellent state of quality — providing clear overviews and intent without overwhelming details or code examples. Docs should orient a reader, not substitute for reading the code. Flag docs that are inaccurate, stale, overly detailed, or missing.

**Critical rule for mismatches:** When a doc and the code disagree, do NOT assume the code is correct. Report both what the code does and what the doc says, and mark the finding as **"Needs human decision"** — the human must determine which should be updated. Never recommend a direction for doc/code mismatches in the report.

**Agent 13: HLD & project-level architecture** (subagent_type: Explore)

Read `docs/architecture/HLD.md`. Then read the root `Cargo.toml` and every crate's `lib.rs` to understand the actual workspace structure. Verify:
- Does the HLD accurately describe the crate topology, their roles, and how they relate?
- Are the architectural principles stated still reflected in the code?
- Is the level of detail appropriate — high-level overview without drowning in implementation specifics?
- Flag any sections that describe components that no longer exist or omit significant new ones.

**Agent 14: Agent architecture docs** (subagent_type: Explore)

Read `docs/architecture/agent/README.md`. Then read `src/agent.rs` thoroughly. Verify:
- Does the doc accurately describe the Agent struct, its construction, event dispatch, and lifecycle?
- Are the described patterns (event subscribers, panic handling, message filtering) still accurate?
- Is the doc appropriately high-level? Flag any excessive code examples or implementation details that belong in rustdoc, not architecture docs.

**Agent 15: Agent loop & context docs** (subagent_type: Explore)

Read `docs/architecture/agent-loop/README.md` and `docs/architecture/agent-context/README.md`. Then read `src/loop_.rs` and `src/context.rs`. Verify:
- Does the loop doc accurately describe the outer/inner loop structure, overflow handling, and turn flow?
- Does the context doc accurately describe sliding window compaction, token budgeting, and tool-result pairing?
- Are the docs oriented toward understanding *intent and design decisions* rather than line-by-line code walkthroughs?

**Agent 16: Streaming & data model docs** (subagent_type: Explore)

Read `docs/architecture/streaming/README.md` and `docs/architecture/data-model/README.md`. Then read `src/stream.rs` and the relevant type definitions. Verify:
- Does the streaming doc accurately describe event accumulation, ordering guarantees, and error handling?
- Does the data model doc accurately describe message types, tool call/result structures, and content blocks?
- Flag any type names or enum variants in the docs that have been renamed or removed.

**Agent 17: Tool system docs** (subagent_type: Explore)

Read `docs/architecture/tool-system/README.md`. Then read `src/tool.rs` and `src/tools/` (if the builtin-tools feature exists). Verify:
- Does the doc accurately describe the AgentTool trait, tool registration, schema generation, and the dispatch pipeline (approval → transformer → validator → schema → execute)?
- Are FnTool, middleware, and dynamic mutation patterns documented if they exist?
- Is the doc useful for someone who wants to *build a custom tool* without excessive internal detail?

**Agent 18: Error handling & eval docs** (subagent_type: Explore)

Read `docs/architecture/error-handling/README.md` and `docs/architecture/eval/README.md`. Then read the corresponding code. Verify:
- Does the error doc accurately describe the error type hierarchy, retry strategy, and retryability rules?
- Does the eval doc accurately describe gates, budgets, auditing, and YAML eval specs?
- Flag any described error variants or eval features that no longer exist.

**Agent 19: TUI docs** (subagent_type: Explore)

Read `docs/architecture/tui/README.md`. Then read the TUI crate's source. Verify:
- Does the doc accurately describe the TUI architecture, agent bridge, and UI components?
- Is the doc useful for someone who wants to understand *how the TUI consumes the core API* at a high level?
- Flag any described UI components, keybindings, or flows that don't match the implementation.

**Agent 20: Planning & phase docs** (subagent_type: Explore)

Read `docs/planning/PRD.md`, `docs/planning/IMPLEMENTATION_PHASES.md`, `docs/planning/EVAL.md`, and `docs/planning/TUI_PHASES.md`. Assess:
- Are these docs still relevant, or have the described phases been completed/superseded?
- Do they accurately describe the project's current direction and priorities?
- Flag any planned features described as "future" that have since been implemented (should be updated or archived).
- Are these docs causing confusion by describing an outdated vision?

**Agent 21: README & getting started docs** (subagent_type: Explore)

Read `README.md`, `docs/getting_started.md`, and `docs/testing_setup.md`. Then verify against the actual project:
- Are setup instructions accurate? Do the described commands actually work?
- Are feature lists and capability claims accurate?
- Is the README appropriately concise — does it orient a newcomer without overwhelming them?
- Does getting_started.md provide a clear, accurate onboarding path?
- Does testing_setup.md reflect the actual test infrastructure?

**Agent 22: AGENTS.md accuracy across workspace** (subagent_type: Explore)

Read every `AGENTS.md` file in the workspace:
- `AGENTS.md` (root)
- `adapters/AGENTS.md`
- `memory/AGENTS.md`
- `src/tools/AGENTS.md`
- `tui/AGENTS.md`
- `tui/src/ui/AGENTS.md`
- `eval/AGENTS.md`
- `local-llm/AGENTS.md`

For each "Lessons Learned" entry, trace it to the actual code and verify the described behavior still exists. These files guide AI-assisted development, so accuracy is critical. Flag:
- Lessons that reference behavior that has changed or been removed.
- Missing lessons for non-obvious patterns discovered since the last update.
- Inconsistencies between nested AGENTS.md files and the root.

---

**Each of Agents 13–22 must produce:**
1. A findings list: inaccuracies, staleness, or quality issues with doc file path and specific section. **Omit docs that are accurate and in good shape — one line "Good: [file]" is sufficient for those.**
2. For any doc needing attention: specific, actionable recommendations for what to fix — keeping the principle that docs should provide overviews and intent, not code-level detail.

## Step 3 — Synthesize results and write QA_Report.md

After all agents complete, write the consolidated QA report to `QA_Report.md` in the project root using the Write tool. Also print a brief summary to stdout.

**Terseness rule:** Every section below should contain ONLY items that need remediation. If a section has no findings, write one line: `All clear.` Do not list what was checked, do not explain why things are fine, do not pad with "the following areas were verified clean." Less is more.

### 3.1 Build & Test Summary
Pass/fail status only. If all pass, one line: `Build & tests: all pass.`

### 3.2 Critical Issues
Anything that would break API consumers or indicates a bug. (Agents 1–3)

### 3.3 Style Violations
Grouped by rule, sorted by severity. Omit any rule with zero violations. (Agent 3)

### 3.4 DRY / Dead Code
Deduplicated actionable items only — within-crate (Agent 2) and cross-crate (Agent 10). No preamble.

### 3.5 Crate Integration & Consumer Experience
Findings only from Agents 5–12. Omit boundaries that are all clear. Include:
- Per-boundary issues (core↔adapters, core↔local-llm, core↔eval, core↔TUI) — only where problems exist
- Cross-cutting DRY concerns
- Consumer experience issues (from custom_agent.rs analysis)
- Architectural risks

### 3.6 Extensibility Gaps
Include the summary table from Agent 4, but **only rows where Impact is Medium or High and a real gap exists**. Drop rows where current state is already adequate.

### 3.7 Documentation Issues
List only docs rated "Needs Update" or "Stale" with specific fixes needed. Omit docs that are accurate.

For each doc/code mismatch, use this format and do not suggest a fix:
> **[Needs human decision]** `file:line` — doc says X, code does Y. Which is the source of truth?

### 3.8 Top Recommendations
Top 10 highest-impact improvements, ordered by priority. Weight extensibility, core-harness fitness, and consumer experience heavily. Each item: one sentence describing the fix + file:line reference.

Use file:line references throughout. No filler. Every sentence in the report should describe something that needs to change.
