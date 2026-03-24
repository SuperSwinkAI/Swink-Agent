# QA Audit Report — Swink-Agent

**Date:** 2026-03-23
**Scope:** Full workspace (all crates)

---

## 3.1 Build & Test Summary

Build & tests: all pass. Clippy (zero warnings), `cargo test --workspace` (all pass), `cargo test -p swink-agent --no-default-features` (all pass).

---

## 3.2 Critical Issues

All clear.

---

## 3.3 Style Violations

**Closure alias naming:**
- `src/orchestrator.rs:24` — `OptionsFactoryArc` should be `OptionsFactoryFn` per convention (closure type aliases suffixed with `Fn`).

**File size:**
- `tui/src/app/tests.rs` — 1978 lines, exceeds ~1500 line guideline. Consider splitting into separate test modules.

---

## 3.4 DRY / Dead Code

**Test helper duplication (within-crate):**

1. `default_convert_to_llm()` — identical implementation in `tests/agent_loop.rs:116`, `tests/fallback.rs:24`, `tests/context_compaction.rs:68`, `tests/tool_execution_policy.rs`. Consolidate into `tests/common/mod.rs`.

2. `default_config()` factory — similar `AgentLoopConfig` boilerplate in `tests/agent_loop.rs`, `tests/fallback.rs:43`, `tests/context_compaction.rs:75`. Extract into `tests/common/mod.rs`.

3. `error_events()` — duplicated in `tests/fallback.rs:31` and `tests/context_compaction.rs:57`. Already exists in `tests/common/mod.rs:384` — remove local copies and import from common.

**Cross-crate:** All clear.

**Dead code:** None found. All `#[allow(dead_code)]` annotations in test helpers are justified.

---

## 3.5 Crate Integration & Consumer Experience

### API Boundary Issues

1. **Missing `is_empty()` methods** — `src/agent_subscriptions.rs:86` (`ListenerRegistry`) and `src/fallback.rs:58` (`ModelFallback`) both have `len()` but no `is_empty()`. Rust convention expects both.

2. **Private return type from public function** — `tui/src/lib.rs:90` (`tui_approval_callback()`) returns `ApprovalCallbackFn` which is a private type alias (line 46). Either make public or use `impl Fn(...)`.

### Core Internal Coherence

All clear. Zero circular references. Clean 5-tier dependency DAG. No responsibility drift.

### Core-Adapters, Core-Eval, Core-Local-LLM, Core-TUI Boundaries

All clear. All boundaries are clean, minimal, and well-defined. Third-party adapters can be written with only a core dependency.

### Consumer Experience (`custom_agent.rs`)

Minor improvement: In `examples/with_tools.rs`, the verbose `Pin<Box<dyn Future>>` approval callback (lines 122-134) is shown as the primary example while the cleaner `with_approve_tool_async` form is buried in a comment (lines 136-141). Swap them so the ergonomic API is primary.

### Architectural Risks

1. **`tokio = { features = ["full"] }` in workspace `Cargo.toml:8`** — Core only needs `sync`, `rt`, `time`, `macros`. Replace with explicit features to reduce compile time and binary size.

2. **Custom message serialization is silently opt-in** — Non-serializable custom messages are silently skipped in checkpoints. Documented in CLAUDE.md but could surprise users.

---

## 3.6 Extensibility Gaps

| Area | Current State | Gap / Opportunity | Impact | Effort |
|------|--------------|-------------------|--------|--------|
| Agent wrapping/decoration | Concrete `Agent` struct; state and lifecycle opaque | No decorator pattern for cross-cutting concerns (tracing, metrics). Must subscribe to events or use hooks | High | Medium |
| Stream error classification | `StreamErrorKind` with 4 variants (Throttled, ContextWindow, Auth, Network) | Missing provider-specific error codes; custom errors require string matching | High | Medium |
| Tool result streaming | `on_update` callback in tool execute (one-way, untyped) | No backpressure, cancellation signal, or structured stream for long-running tool outputs | Medium | Large |
| Tool execution policies | `ToolExecutionPolicy` trait with strategy variants | Cannot inject per-tool custom executors or rewire tool discovery | Medium | Large |
| Message provider priority | `MessageProvider` trait (poll-based) + `ChannelMessageProvider` | No hierarchical priority queues or backpressure signaling | Medium | Medium |
| Post-turn hooks | `AsyncContextTransformer` for pre-turn I/O | Missing post-turn hooks for summary storage/indexing | Medium | Small |
| Loop policy composition | `ComposedPolicy` with AND semantics only | Cannot express OR/weighted policies; no access to tool results during decision | Medium | Small |

---

## 3.7 Documentation Issues

### Needs Update

**`README.md` — Code example outdated:**
- Shows `Arc::new(BashTool::new())` instead of current `BashTool::new().into_tool()` pattern.
- Missing `use swink_agent::prelude::*;` import.
- Should match actual `examples/custom_agent.rs`.

**`docs/getting_started.md` — Library usage example outdated (lines 125-144):**
- Shows old multi-parameter `AgentOptions::new()` constructor that no longer exists.
- Update to match current `AgentOptions` builder API.

**`docs/architecture/tool-system/README.md` — Two gaps:**
- Missing `.with_schema(Value)` in FnTool builder API table.
- `IntoTool` trait not documented despite being a primary public API.

**`docs/planning/TUI_PHASES.md` — Phase T5 misleading:**
- Only two items remain (side-by-side diffs, per-hunk approve/reject), both optional polish.
- Should clarify these are nice-to-haves, not blocking.

**`docs/architecture/HLD.md:10` — Minor:** Says "seven crates" without distinguishing xtask from the 6 library/app crates. Clarify.

### Needs Human Decision

> **[Needs human decision]** `docs/architecture/agent/README.md` (Event Forwarders section) — doc says forwarders are "suitable for queueing async tasks", code shows they are synchronous `Fn(AgentEvent)` closures that block dispatch. Which is the source of truth?

---

## 3.8 Top Recommendations

1. **Update `README.md` and `docs/getting_started.md` code examples** to use current API (`IntoTool`, prelude imports, current `AgentOptions` builder). These are the first things new users see. — `README.md:40-74`, `docs/getting_started.md:125-144`

2. **Replace `tokio = { features = ["full"] }` with explicit features** (`sync`, `rt`, `time`, `macros`) in workspace `Cargo.toml:8`. Reduces compile time and binary size.

3. **Consolidate test helpers** — move `default_convert_to_llm()`, `default_config()`, and remove duplicate `error_events()` into `tests/common/mod.rs`. Four files affected.

4. **Document `IntoTool` trait** in tool system architecture doc — it's the primary API for tool registration but undocumented. — `docs/architecture/tool-system/README.md`

5. **Rename `OptionsFactoryArc` to `OptionsFactoryFn`** per closure alias naming convention. — `src/orchestrator.rs:24`

6. **Add `is_empty()` methods** to `ListenerRegistry` and `ModelFallback` alongside existing `len()`. — `src/agent_subscriptions.rs:86`, `src/fallback.rs:58`

7. **Make `with_approve_tool_async` the primary example** in `examples/with_tools.rs`, moving the verbose `Pin<Box<...>>` form to an "Advanced" comment. — `examples/with_tools.rs:122-141`

8. **Resolve event forwarder doc/code mismatch** — decide whether forwarders should support async or update doc to clarify sync-only nature. — `docs/architecture/agent/README.md`, `src/event_forwarder.rs:6`

9. **Split `tui/src/app/tests.rs`** (1978 lines) into focused test modules per the ~1500 line guideline.

10. **Consider `AgentDecorator` trait** for cross-cutting concerns (tracing, metrics, filtering) at the Agent level — currently requires subscribing to multiple event points. — Core extensibility gap.
