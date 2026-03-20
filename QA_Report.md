# QA Report — Swink-Agent — 2026-03-14

## 3.1 Build & Test Summary

| Command | Result |
|---|---|
| `cargo clippy --workspace -- -D warnings` | PASS — zero warnings |
| `cargo test --workspace` | PASS — all tests pass; minor unused-import warnings in `tests/agent_structured.rs:5,11,14,17-18` and `local-llm/src/preset.rs:55` (not errors) |
| `cargo test -p swink-agent --no-default-features` | PASS |

---

## 3.2 Critical Issues

No API-breaking bugs found. All public trait signatures, constructor patterns, and crate boundaries are correct and stable.

---

## 3.3 Style Violations

### Rule: File size — "split at ~1500 lines"

- `src/agent.rs` — 1,841 lines; mixes `SubscriptionId`, `SteeringMode`/`FollowUpMode` enums, `AgentState`, `AgentOptions` (with builder chain), `Agent` implementation, and event subscription management.

### Rule: Test names — "descriptive snake_case without `test_` prefix"

The following test files contain functions prefixed with `test_`, violating the convention:

| File | Count | Example violation |
|---|---|---|
| `tests/agent_loop.rs` | 18 | `test_3_1_single_turn_no_tool` |
| `tests/integration.rs` | 16 | `test_6_4_steering_interrupts_tool_execution` |
| `tests/agent.rs` | 10 | (various) |
| `tests/agent_steering.rs` | 6 | (various) |
| `tests/agent_structured.rs` | 6 | (various) |
| `tests/agent_continuation.rs` | 8 | (various) |
| `tests/loop_policy.rs` | 1 | (various) |
| `tests/stream_middleware.rs` | 2 | (various) |
| `tests/sub_agent.rs` | 1 | (various) |

### Rule: Predicates — `is_*`/`has_*` prefix

- `src/types.rs:231` — `CustomMessageRegistry::contains()` returns `bool` but does not carry the `is_`/`has_*` prefix. Rename to `has_type_name()` or `has_registered_deserializer()` for consistency.

---

## 3.4 DRY / Dead Code

### Duplicated test helpers

| Duplication | Files | Recommended action |
|---|---|---|
| `ContextCapturingStreamFn` (identical struct + impl) | `tests/agent_continuation.rs:22-63`, `tests/agent_loop.rs:25-38`, `tests/agent_steering.rs:24-65`, `tests/context_compaction.rs:24-65` | Move to `tests/common/mod.rs` |
| `ApiKeyCapturingStreamFn` (identical) | `tests/agent_continuation.rs:342-374`, `tests/agent_loop.rs:40-52` | Move to `tests/common/mod.rs` |
| `default_model()` and `user_msg()` | `tests/fallback.rs:17-39` (duplicates `tests/common/mod.rs:207-229`) | Remove from `fallback.rs`; import from `common` |

### Cross-crate duplication (local-llm)

- `LocalModel` (`local-llm/src/model.rs`) and `EmbeddingModel` (`local-llm/src/embedding.rs`) are near-identical wrappers: same `ModelState`/`EmbeddingModelState` enum variants, same `Arc<Inner>` pattern, same `ensure_ready()` logic, same `with_progress()` footgun guard. ~400 lines of duplication. Extract a generic `LazyModel<C, S>` wrapper.

### Cross-crate duplication (adapters)

- All six remote adapters (`anthropic.rs`, `openai.rs`, `bedrock.rs`, `azure.rs`, `google.rs`, `mistralrs.rs`) repeat the same `new(base_url, api_key)` constructor and redacted `Debug` impl. Extract a `BaseStreamFn { base_url, api_key, client }` struct for composition.

- `adapters/src/anthropic.rs:316-400` defines its own `convert_messages` function that duplicates the generic pattern in `src/convert.rs:46-72`, differing only to handle the top-level system field and thinking block filtering. Either extend `MessageConverter` with `filter_content_blocks()` and `system_placement()` hooks, or document as an intentional justified exception.

---

## 3.5 Crate Integration & Consumer Experience

### Core internal coherence (Agent 5)

- `loop_/tool_dispatch.rs:72-147` — The approval→transformer→validator→schema gate sequence is hardcoded; no plugin point to reorder or insert stages without modifying the dispatch function.
- `loop_/turn.rs:52-69` — Two-stage transformation (async then sync) and the overflow signal reset are implicit; not surfaced in any event or hook.
- `types.rs` — `CustomMessage` can be appended to history but is silently filtered from LLM context with no hook for custom-to-LLM conversion.
- Event emission occurs ~20+ times across loop modules with no centralized policy; ordering guarantees are convention, not enforced.

### Core ↔ Adapters

- `adapters/src/lib.rs:2,5` — `pub mod classify` and `pub mod sse` expose implementation utilities with no documented stability contract; downstream code may depend on them and break silently. Add module-level stability notes.
- No third-party adapter guide exists; document that external StreamFn implementors should depend only on `swink_agent` (not `swink_agent_adapters`).

### Core ↔ LocalLLM

- `local-llm/src/model.rs:135-142` and `local-llm/src/embedding.rs:108-114` — `with_progress()` returns `Err` if called after `Arc::clone()`; this footgun is only documented in `CLAUDE.md`, not in the method's doc comment. Add `/// Must be called before cloning the Arc.` to both.
- `local-llm/src/preset.rs:22-51` — `default_local_connection()` calls `model_catalog()` from core, making local-llm unbootstrappable without the core catalog. Extracting preset loading to a local config file would decouple them.

### Core ↔ Eval

- `eval/src/efficiency.rs:111-153` — Defines its own `make_invocation()` inline; should use `eval/tests/common/mod.rs` helpers instead. `eval/src/audit.rs:88-101` similarly inlines `AssistantMessage` construction that duplicates test helpers.
- `eval/src/gate.rs:31,38,45` — `with_min_pass_rate()` and similar builders accept any `f64` including values outside `[0.0, 1.0]`; no validation.
- `BudgetEvaluator` is post-hoc only; no shared entry point for external harnesses to reuse mid-execution budget cancellation (`BudgetGuard`) without duplicating the `collect_with_guard()` call pattern.

### Core ↔ TUI

- `src/agent.rs:1616` — `handle_stream_event()` is the required state-sync call for streaming mode, but its doc comment does not explain this requirement. Any new frontend that uses `prompt_stream()` will silently fail to update `agent.state()` unless they discover this call. Add a doc example showing the channel loop pattern.
- `src/agent.rs:85-86` — `PLAN_MODE_ADDENDUM` is a hardcoded `const &str`; different frontends (CLI, web) cannot customize planning instructions. Move to `AgentOptions::plan_mode_addendum` with a `with_plan_mode_addendum()` builder.
- `tui/src/app/state.rs:57-74` — `DisplayMessage` duplicates the message hierarchy for rendering; every future frontend must rebuild this transformation. Consider a `FrontendMessageAdapter` trait in core to make the pattern reusable.

### Consumer experience (custom_agent.rs example)

- `examples/custom_agent.rs:13` — Imports `AgentTool` (a trait) that is never used directly in the example; remove or comment on why it is needed.
- `examples/custom_agent.rs:23-29` — `ModelConnections` decomposition semantics (how `from_connections()` decomposes the struct internally) are invisible; add an inline comment.
- No "minimal real provider" example exists between `simple_prompt.rs` (mocked) and `custom_agent.rs` (full TUI + adapters). A `minimal_agent.rs` (~30 lines, one provider, no TUI) would bridge the gap.
- `README.md:68-75` — The README "Build a Custom Agent" example uses the old `connections.into_parts()` + `AgentOptions::new()` API; update to use `AgentOptions::from_connections()` as shown in `examples/custom_agent.rs:39-41`.

### Architectural risks

- `src/stream.rs:80-138` — `AssistantMessageEvent` uses positional content-block indices throughout the accumulator; adding a new block type requires coordinated changes in the enum, `accumulate_message()`, every adapter, and the loop. Start planning an extensible event registry or versioning strategy before new block types are added.
- `src/agent.rs:1395+` — `dispatch_event` uses `catch_unwind` and auto-removes panicking subscribers; `ModelCycled` (when the agent silently switches models on a retryable error) emits no event, creating an observability blind spot. Emit a `ModelCycled { old, new, reason }` event.
- `tui/src/ui/tool_panel.rs:119` — Tool panel auto-fade is 3 seconds in code; `tui/CLAUDE.md:45` documents it as 10 seconds. Either fix the code or the doc.
- `tui/CLAUDE.md:9` — Documents provider priority as "Proxy > OpenAI > Anthropic > Ollama" but `tui/src/credentials.rs` iterates in order Ollama → OpenAI → Anthropic. Verify and correct.

---

## 3.6 Extensibility Gaps

| Area | Current State | Gap / Opportunity | Impact | Effort |
|---|---|---|---|---|
| Custom Message Persistence | `CustomMessage` excluded from `Checkpoint` and `SessionStore` | Enable serialization of custom messages in persistence layer | High | Medium |
| Model Cycling Events | Agent silently cycles on retryable errors; no event emitted | Add `ModelCycled { old, new, reason }` to `AgentEvent` | High | Small |
| Agent Registry / Peer Discovery | Orchestrator requires manual child spawning via factory | Add `AgentRegistry` for named agent lookup; enable dynamic agent networks | High | Medium |
| Post-Tool Execution Hook | Tool results are final; no enrichment or error-handling hook | Add `ToolResultTransformer` trait for post-execution result processing | Medium | Small |
| Incremental Checkpointing | Pause requires loop interruption | Add async checkpoint hook after each turn completion | Medium | Medium |
| Inter-Agent Messaging | No peer-to-peer; must route through orchestrator | Add optional `AgentMailbox` trait for direct agent-to-agent channels | Medium | Medium |

---

## 3.7 Documentation Issues

### Streaming & data model — Accurate.

### Tool system

- `docs/architecture/tool-system/README.md` (near the end) — `on_update` callback: the doc says "reserved for future use" but does not explain that agent loop always passes `None` and that custom tools should guard with `if let Some(f) = on_update`. Add this guidance so implementors don't write unreachable streaming code.
- Missing a "Building a Custom Tool" step-by-step section showing both the `FnTool` closure pattern and the struct-based `impl AgentTool` pattern side by side.

### Error handling

- `docs/architecture/error-handling/README.md:67-78` — `AgentError::BudgetExceeded` variant exists in `src/error.rs:52-53` but is absent from the taxonomy diagram and trigger list. Add it.

### Eval

- `docs/architecture/eval/README.md:155-191` — `EfficiencyEvaluator` is loaded by `EvaluatorRegistry::with_defaults()` (`eval/src/evaluator.rs:69`) but is not documented in the "Built-in Evaluators" subsection alongside `TrajectoryMatcher`, `BudgetEvaluator`, and `ResponseMatcher`. Add a subsection for it.

### Agent architecture

- `docs/architecture/agent/README.md` — Event forwarders (`event_forwarders: Vec<EventForwarderFn>`, `add_event_forwarder()`, `with_event_forwarder()`) are not mentioned. The "listener registry" flowchart should note that async event forwarders run after synchronous listeners.

### Loop & context

- `docs/architecture/agent-loop/README.md` and `docs/architecture/agent-context/README.md` — Both reference `src/loop_.rs` as a single file, but the implementation is a module directory `src/loop_/` with `mod.rs`, `stream.rs`, `tool_dispatch.rs`, and `turn.rs`. Update all file path references.
- `docs/architecture/agent-context/README.md` — States source files are `src/types.rs` and `src/context_transformer.rs`; the core compaction logic lives in `src/context.rs`. Add `src/context.rs` as primary reference.

### HLD

- `docs/architecture/HLD.md:359` — Workspace dependency diagram shows `loop_["loop_.rs<br/>..."]` as a single file; update to `loop_/ (module)` with submodule list.
- `docs/architecture/HLD.md:67-68` — Adapter table lists 8 adapters in one row and mentions `ProxyStreamFn` separately; merge into a single row listing all nine.

### TUI

- `docs/architecture/tui/README.md:44-45` — States tool panel auto-fades after "10s"; code (`tui/src/ui/tool_panel.rs:119`) uses 3 seconds. Fix the doc (or fix the code if 10s was intended).
- `docs/architecture/tui/README.md:299-344` — "Inline Diff View" section documents y/n/a hunk-approval keybindings and hunk navigation that are not implemented. Either remove this section or mark it as "Phase T5 — not yet implemented".
- `docs/architecture/tui/README.md:238-246` — Configuration table is missing the `system_prompt: Option<String>` field (`tui/src/config.rs:20`). Add it.

### README / Getting Started

- `README.md:68-75` — Code example uses `connections.into_parts()` + `AgentOptions::new()` (old API). Update to `AgentOptions::from_connections()`.
- `.env.example:27` — `OPENAI_MODEL=gpt-5.2` references a non-existent model ID; replace with `gpt-4o`.

### CLAUDE.md accuracy

- `tui/CLAUDE.md:9` — Provider priority "Proxy > OpenAI > Anthropic > Ollama" does not match the iteration order in `tui/src/credentials.rs` (Ollama → OpenAI → Anthropic). Verify intended order and correct.
- `local-llm/src/model.rs:135-142` and `local-llm/src/embedding.rs:108-114` — The `with_progress()` footgun ("call before cloning the Arc") is only in `CLAUDE.md`; add it to the method's rustdoc comment directly.

---

## 3.8 Top Recommendations

1. **Add `ModelCycled` event to `AgentEvent`** (`src/agent.rs`) — Closes an observability blind spot; the agent silently switches models on retryable errors with no way for subscribers, metrics collectors, or the TUI to observe the switch. Small effort, high value.

2. **Document `handle_stream_event()` as required for streaming mode** (`src/agent.rs:1616`) — Any frontend using `prompt_stream()` must call this to keep `agent.state()` in sync, but the method has no doc comment explaining this. Add a rustdoc example showing the channel loop. Prevents silent bugs in every future frontend.

3. **Fix test function naming (`test_` prefix) across integration tests** (`tests/agent_loop.rs`, `tests/integration.rs`, and 7 others) — 68+ violations of the project's own style rule. Rename by dropping the `test_` prefix (sed or cargo-fix can batch-rename).

4. **Split `src/agent.rs`** (1,841 lines) — Extract `AgentOptions` + builders into `src/agent_options.rs`, and event subscription logic into `src/agent_subscriptions.rs`, leaving `agent.rs` under 1,200 lines.

5. **Consolidate duplicated test helpers** (`tests/agent_continuation.rs`, `tests/agent_loop.rs`, `tests/agent_steering.rs`, `tests/context_compaction.rs`) — Move `ContextCapturingStreamFn` and `ApiKeyCapturingStreamFn` to `tests/common/mod.rs`; delete local definitions from `tests/fallback.rs:17-39`.

6. **Enable `CustomMessage` persistence in `Checkpoint` and `SessionStore`** (`src/checkpoint.rs`, `memory/src/`) — Custom messages are silently dropped on save/load; add a JSON envelope and registry-based deserialization to allow round-trip persistence.

7. **Add `AgentError::BudgetExceeded` to the error taxonomy doc** (`docs/architecture/error-handling/README.md:67-78`) — The variant exists in `src/error.rs:52-53` but is absent from the diagram; any developer reading the doc will not know this error exists.

8. **Extract `BaseStreamFn` composition struct for adapters** (`adapters/src/`) — Six adapters copy the same `new(base_url, api_key)` constructor and redacted `Debug` impl; ~100 lines of dead duplication. Add `BaseStreamFn` and have each adapter embed it.

9. **Add `with_plan_mode_addendum()` to `AgentOptions`** (`src/agent.rs:85-86`) — The plan-mode system prompt addendum is a hardcoded `const`; moving it to a builder option lets CLI and web frontends customize planning instructions without forking the agent.

10. **Extract generic `LazyModel<C, S>` in `local-llm`** (`local-llm/src/model.rs`, `local-llm/src/embedding.rs`) — `LocalModel` and `EmbeddingModel` are near-identical ~200-line wrappers differing only in the runner type; a single generic eliminates the duplication and any future divergence risk.
