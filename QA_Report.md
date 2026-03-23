# QA Report — Swink-Agent — 2026-03-23

## 3.1 Build & Test Summary

| Command | Result |
|---------|--------|
| `cargo clippy --workspace -- -D warnings` | **PASS** |
| `cargo test --workspace` | **1 FLAKY** — `concurrent_tool_execution` timing assertion (132ms > 130ms threshold) |
| `cargo test -p swink-agent --no-default-features` | **PASS** |

**Compiler warnings (tests only, not blocking):** 8 unused imports across test files (`ModelSpec` in loop_policy.rs:9, sub_agent.rs:13, stream_middleware.rs:11; `Mutex` in agent_steering.rs:5; `Pin`/`Stream`/`CancellationToken`/multiple types in agent_structured.rs:5-20). 3 dead code warnings in local-llm/tests/common/mod.rs (`ProgressCollector`, `test_model_config`). 1 unused import in local-llm/src/preset.rs:138.

## 3.2 Critical Issues

**Flaky test:** `tests/ac_tools.rs:277` — `concurrent_tool_execution` asserts elapsed < 130ms but measured 132ms. Timing-based assertion is inherently fragile; widen threshold or use a concurrency proof (e.g., verify tasks overlapped rather than checking wall-clock time).

## 3.3 Style Violations

### Import order (76 files)

**Rule:** `std` → external (alphabetical) → `crate::`/`super::`.

Imports are interspersed across 76 files in all crates. Notable examples:
- `src/agent.rs:11-20` — std mixed with external crates
- `tui/src/app/tests.rs:1-21` — `std::future::Future` after external imports
- `adapters/src/anthropic.rs:7-18` — std then external then swink_agent

Full list spans core (21 files), adapters (10), memory (2), local-llm (1), eval (3), TUI (6), tests (27), examples (3).

### Mock naming (9 instances)

**Rule:** Test mocks prefixed with `Mock`.

- `tests/common/mod.rs:23` — `FlagStreamFn` (not `MockFlagStreamFn`)
- `tests/common/mod.rs:95` — `ContextCapturingStreamFn`
- `tests/common/mod.rs:143` — `ApiKeyCapturingStreamFn`
- `tests/agent_loop.rs:27` — `UpdatingTool`
- `tests/agent_loop.rs:984` — `PanickingTool`
- `tests/ac_tools.rs:26` — `ArgCapturingTool`
- `tests/context_compaction.rs:26` — `MessageCapturingStreamFn`
- `tests/model_connections.rs:12` — `DummyStreamFn`
- `tests/integration.rs:30` — `TransformTrackingStreamFn`

## 3.4 DRY / Dead Code

### Dead code annotations (future-use functions)

- `local-llm/src/convert.rs:83` — `convert_tools()` marked `#[allow(dead_code)]` ("will be used when tool calling wired in")
- `local-llm/src/convert.rs:117` — `tool_schemas_json()` same
- `tui/src/credentials.rs:89` — `delete_credential()` ("reserved for future credential UI")
- `tui/src/ui/input.rs:250` — `is_empty()` ("reserved for input validation")
- `tui/src/ui/conversation.rs:70` — `scroll_to_bottom()` ("reserved for keyboard shortcut")
- `tui/src/ui/tool_panel.rs:110` — `has_pending_approval()`
- `tui/src/theme.rs:93,207,216` — `role_color()`, `mono_fg()`, `mono_bg()`

**Action:** Track via issues or remove if abandoned.

### Duplicate MockTool

- `local-llm/src/convert.rs:153-193` duplicates `tests/common/mod.rs:191-297`. Import from core's test-helpers feature instead.

### Unused test-helpers feature

- `Cargo.toml:56` defines `test-helpers` feature gating `src/testing.rs`, but no downstream crate enables it. Clarify intent or remove.

### Adapter duplication (within-crate, medium priority)

- `parse_sse_stream()` reimplemented in 5 adapters (anthropic, openai, azure, google, proxy). SSE frame parsing could be consolidated further in `adapters/src/sse.rs`.
- HTTP status → error event mapping duplicated across 5 adapters despite `classify.rs` existing. Consolidate into a shared helper.
- `send_request()` pattern (URL construction + logging + POST + error handling) repeated in 6 adapters.

### Test mock response-fetching

- `tests/common/mod.rs` — identical `if responses.is_empty()` + fallback logic copied across 4 StreamFn mock implementations (lines 37-46, 74-87, 122-135, 170-183). Extract helper.

## 3.5 Crate Integration & Consumer Experience

### Core internal coherence
All clear. Acyclic dependency graph, clean module boundaries, no circular references.

### Core ↔ Adapters
All clear. Third-party adapters can be written against core only. Internal utilities (`classify`, `sse`, `finalize`) documented as unstable.

### Core ↔ Local-LLM
All clear. Cleanly implements `StreamFn` and `MessageConverter` traits. Fully swappable.

### Core ↔ Eval
All clear. Consumes only public API. Evaluator trait is open for external implementations.

### Core ↔ TUI
All clear. Agent bridge is exemplary consumer of core API. Alternative frontends (web, CLI, GUI) could be built with same ease.

### Consumer experience (from custom_agent.rs analysis)

**Key friction points:**

1. **Result text extraction** — Every consumer must pattern-match `AgentMessage::Llm(LlmMessage::Assistant(...))` + `ContentBlock::extract_text()`. Add `AgentResult::assistant_text()` convenience method.

2. **Mock boilerplate** — Examples duplicate ~30 lines of `MockStreamFn` setup. Export reusable mock from `testing` module.

3. **Event sequence ceremony** — Building `AssistantMessageEvent` sequences requires 5+ lines with manual `content_index` tracking. Add `AssistantMessageEvent::text_response(text)` builder.

4. **Import count** — `with_tools.rs` requires 16 imports. Examples should demonstrate `use swink_agent::prelude::*`.

5. **Type count** — Consumer must understand ~15 types for a basic agent. Acceptable for a full-featured framework, but prelude usage would reduce perceived burden.

### Architectural risks
All clear. Dependencies minimal and appropriate. Feature flags sustainable. Tokio coupling is loose (public API uses `futures::Stream`). JSONL serialization not a lock-in (`SessionStore` trait abstracts it). Error hierarchy consistent and extensible.

## 3.6 Extensibility Gaps

| Area | Current State | Gap / Opportunity | Impact | Effort |
|------|--------------|-------------------|--------|--------|
| Plugin/wrapper architecture | Agent fields private; `AgentHandle` for spawned access | Cannot wrap Agent to intercept prompt calls; `AgentLoopConfig` is internal | H | M |
| Agent wrapping | State accessible via `.state()` | Queues (steering, follow_up) are private `Arc<Mutex>` — external loop control blocked | H | M |
| Thread-local agent state | No trait for agent-scoped context storage | External crates cannot attach scoped state to agent lifecycle | H | S |
| Multi-agent coordination | AgentOrchestrator (master/worker) | No peer messaging, dynamic agent discovery, or workload queue abstraction | M | L |
| Lifecycle events | 21+ AgentEvent variants | Events are sync-only; no async hooks for persistence at TurnEnd | M | M |
| Tool system | Fully pluggable via AgentTool trait | Missing per-tool rate limiting; `requires_approval()` bool insufficient for context-aware approval | M | S |
| Loop inspection | TurnEnd carries TurnSnapshot | Cannot pause mid-turn; PolicyContext is read-only | M | S |
| Cancellation | CancellationToken propagated everywhere | No graceful shutdown (drain-pending-tools hook); no whole-loop timeout | M | S |
| Budget control | BudgetGuard for mid-call gating | No per-tool budget/quota; overflow recovery forces full re-context | M | M |
| Testing support | test-helpers feature, MockStreamFn | No test doubles for CheckpointStore, MessageProvider; no conversation replay runner | M | M |

## 3.7 Documentation Issues

### Needs Update

**`docs/architecture/data-model/README.md`** — Missing `extra: HashMap` field on Usage and Cost types. Also missing Extension variant's permitted message roles in diagram.

**`docs/architecture/agent-loop/README.md`** — Config diagram omits `async_transform_context`. Doc references `get_steering_messages` closure; code uses `MessageProvider::poll_steering()` trait method.

**`docs/architecture/agent/README.md`** — States event forwarders are "suitable for async side effects" but `EventForwarderFn` is `Arc<dyn Fn(AgentEvent) + Send + Sync>` (synchronous).

**`docs/architecture/eval/README.md`** — Missing sections for: gate system (`gate.rs`), audit trails (`audit.rs`), budget guarding (`collect_with_guard()`), YAML loading (`yaml.rs`).

**`docs/architecture/tui/README.md`** — Dependency versions stale (ratatui 0.29→0.30, crossterm 0.28→0.29). Diff feature status misleading ("not yet implemented" but display IS implemented; only hunk-level approval is pending). Missing `#approve untrust` commands.

**`docs/architecture/HLD.md`** — Missing new public API modules (`display.rs`, `message_provider.rs`, `event_forwarder.rs`). TUI component diagram missing Help Panel and Diff visualization. Eval section lists only 3 built-in evaluators; crate exports 5.

**`docs/planning/EVAL.md`** — Misleading: conflates completed foundation with aspirational features. Lists TrajectoryCollector as `[P]` Partial but it's fully implemented. High confusion risk.

**`docs/planning/TUI_PHASES.md`** — Phase T5 overstated as "planned" but plan mode, approval, and trust are already implemented (spec 029). Only side-by-side diff and per-hunk approval remain.

### Needs Human Decision

> **[Needs human decision]** `docs/architecture/data-model/README.md` — doc omits `Usage.extra` and `Cost.extra` fields. Code has `extra: HashMap<String, u64>` and `extra: HashMap<String, f64>` respectively (`src/types.rs:349,407`). Should the diagram be updated?

> **[Needs human decision]** `docs/architecture/agent-loop/README.md` — doc shows `get_steering_messages: Fn()` in config; code uses `MessageProvider` trait with `poll_steering()` and `poll_follow_up()` methods. Which should be updated?

> **[Needs human decision]** `docs/architecture/agent/README.md:172` — doc says forwarders suitable for "async side effects"; code signature is synchronous `Fn(AgentEvent)`. Clarify doc or make forwarders async?

### Good (no action needed)

- `docs/architecture/streaming/README.md` — Accurate
- `docs/architecture/tool-system/README.md` — Fully accurate, excellent for custom tool builders
- `docs/architecture/error-handling/README.md` — All error variants and retry rules match code
- `docs/architecture/agent-context/README.md` — Accurate (sliding window, overflow, tool-result pairing)
- `README.md` — Accurate and concise
- `docs/getting_started.md` — Accurate onboarding path
- `docs/testing_setup.md` — All commands verified working
- All CLAUDE.md files — 55/55 lessons verified accurate

## 3.8 Top Recommendations

1. **Add `AgentResult::assistant_text()` convenience method** — eliminates 5-line pattern-match boilerplate in every consumer. `src/agent.rs` or new result type.

2. **Fix flaky `concurrent_tool_execution` test** — `tests/ac_tools.rs:277`. Replace wall-clock assertion with concurrency proof (e.g., overlapping timestamps).

3. **Expose `AgentLoopConfig` or create public `TurnRunner` trait** — enables external loop implementations and experimentation. `src/loop_/mod.rs`.

4. **Export `MockStreamFn` and event builders from `testing` module** — saves ~30 LOC per example/test. `src/testing.rs` + enable `test-helpers` feature in dev-deps.

5. **Consolidate adapter HTTP error classification** — 5 adapters duplicate status→event mapping despite `classify.rs` existing. `adapters/src/classify.rs`.

6. **Add eval doc sections for gate, audit, budget guard, YAML** — significant implemented features are undocumented. `docs/architecture/eval/README.md`.

7. **Update TUI doc dependency versions and diff feature status** — stale versions and misleading "not implemented" claim. `docs/architecture/tui/README.md`.

8. **Add `AssistantMessageEvent::text_response(text)` builder** — reduces mock event ceremony from 5 lines to 1. `src/stream.rs`.

9. **Standardize import order across workspace** — 76 files violate `std → external → crate` convention. Batch fix with `cargo fmt` custom rules or manual pass.

10. **Deprecate or rewrite `docs/planning/EVAL.md`** — conflates completed features with aspirational roadmap, high confusion risk for contributors.
