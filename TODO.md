# TODO

QA audit findings from 2026-03-13. Organized by priority with severity levels.

**Severity key:** `CRITICAL` = safety/correctness issue, `HIGH` = significant quality/usability gap, `MEDIUM` = maintainability/consistency concern, `LOW` = minor improvement
**Effort key:** S = small (< 1 day), M = medium (1-3 days), L = large (3+ days)

---

## P0 — Fix Now

### Code Fixes

| Severity | Area | Item | Effort | File(s) |
|----------|------|------|--------|---------|
| CRITICAL | Safety | Add `#![forbid(unsafe_code)]` to memory crate root — all other 5 crates have it | S | `memory/src/lib.rs:1` |
| HIGH | Style | Split `loop_.rs` (1686 lines) — exceeds 1500-line threshold. Factor out turn execution, tool dispatch, stream handling | M | `src/loop_.rs` |
| MEDIUM | Style | Fix import order in `loop_.rs` — `use crate::message_provider::MessageProvider` separated from main crate import block | S | `src/loop_.rs:56` |
| MEDIUM | Style | Split `tests/agent.rs` (1532 lines) into agent_construction, agent_streaming, agent_tools, agent_control | S | `tests/agent.rs` |

### CLAUDE.md Fixes

| Severity | Area | Item | Effort | File(s) |
|----------|------|------|--------|---------|
| HIGH | Accuracy | Remove or fix `biased;` lesson in TUI CLAUDE.md — TUI event loop does NOT use `biased;` in `tokio::select!` | S | `tui/CLAUDE.md` |
| MEDIUM | Accuracy | Fix conversation.rs comment from "collapsed" to "dimmed" to match TUI UI CLAUDE.md lesson | S | `tui/src/ui/conversation.rs` |

---

## P1 — Documentation Accuracy

### Architecture Docs (Inaccurate Content)

| Severity | Area | Item | Effort | File(s) |
|----------|------|------|--------|---------|
| HIGH | HLD | Update HLD.md — missing `local-llm` crate entirely, only lists 4/9 adapters, missing subsystems (catalogs, registries, policies, middleware, messaging) | M | `docs/architecture/HLD.md` |
| HIGH | Data Model | Fix `ThinkingLevel::XHigh` → `ExtraHigh`; add undocumented `ContentBlock::Extension` variant; add `ToolResultMessage.details` field | S | `docs/architecture/data-model/README.md` |
| HIGH | Streaming | Add `StreamErrorKind` enum documentation; add `Cost` field to Done/Error events; fix incomplete event signatures | S | `docs/architecture/streaming/README.md` |
| HIGH | Error Handling | Add missing `Plugin` error variant to taxonomy diagram; complete retryability list (add Plugin, NoMessages, InvalidContinue) | S | `docs/architecture/error-handling/README.md` |
| HIGH | Tool System | Add documentation for FnTool, ToolMiddleware, ToolValidator, ToolCallTransformer — 4 major public APIs completely undocumented. Add complete dispatch pipeline diagram | M | `docs/architecture/tool-system/README.md` |
| MEDIUM | Agent | Update `continue_loop()` references → `continue_stream/async/sync()` (3 concrete methods) | S | `docs/architecture/agent/README.md` |
| MEDIUM | Agent Loop | Add note that `transform_context` is synchronous (not async) | S | `docs/architecture/agent-loop/README.md` |
| MEDIUM | Agent Context | Fix source file reference (should reference both `types.rs` and `context.rs`, not just `types.rs`) | S | `docs/architecture/agent-context/README.md` |
| MEDIUM | Eval | Add `EfficiencyEvaluator` to `with_defaults()` evaluator list (currently lists 3, should list 4) | S | `docs/architecture/eval/README.md` |
| MEDIUM | TUI | Fix auto-collapse timing (doc says 3s, actual is 10s); remove `/model` command (doesn't exist, use F4); remove `default_approval_mode` config field (doesn't exist) | S | `docs/architecture/tui/README.md` |

### User-Facing Docs

| Severity | Area | Item | Effort | File(s) |
|----------|------|------|--------|---------|
| MEDIUM | README | Add `swink-agent-local-llm` and `swink-agent-eval` to workspace table; update adapter list (add Azure, xAI, Mistral, Bedrock) | S | `README.md` |
| MEDIUM | Getting Started | Fix model version inconsistencies (`claude-sonnet-4-20250514` → `claude-sonnet-4-6`); expand provider list | S | `docs/getting_started.md` |
| MEDIUM | Testing Setup | Fix model version; add local-llm setup instructions; clarify thinking mode availability per provider | S | `docs/testing_setup.md` |

### Planning Docs

| Severity | Area | Item | Effort | File(s) |
|----------|------|------|--------|---------|
| MEDIUM | PRD | Update crate structure (3 → 6 crates); update adapter list; downgrade unimplemented TUI features (per-hunk diff, external editor, collapsible blocks, session trust) | M | `docs/planning/PRD.md` |
| MEDIUM | Impl Phases | Add post-phase sections for memory, local-llm, eval crates and adapter expansion | S | `docs/planning/IMPLEMENTATION_PHASES.md` |
| MEDIUM | Eval Plan | Fix unreliable status markers; clarify scope (active roadmap vs historical reference) | S | `docs/planning/EVAL.md` |
| MEDIUM | TUI Phases | Add Phase T5 for unimplemented features; fix acceptance criteria that claim completion for unbuilt features (AC-21/22/24/26/28/29/30) | M | `docs/planning/TUI_PHASES.md` |

---

## P2 — DRY / Code Consolidation

### Adapter Duplication (High Priority)

| Severity | Area | Item | Effort | File(s) |
|----------|------|------|--------|---------|
| HIGH | Adapters | Consolidate SSE line parsing — Azure, OpenAI reimplement `sse_data_lines` instead of using shared `SseStreamParser` from `sse.rs`. ~200 LOC duplicated | M | `adapters/src/{azure,openai,google}.rs`, `adapters/src/sse.rs` |
| HIGH | Adapters | Extract shared `finalize_blocks` — 5 adapters reimplement identical end-of-stream block-closing logic. ~150 LOC duplicated | M | `adapters/src/{anthropic,azure,openai,google,ollama}.rs` |
| MEDIUM | Adapters | Extract OpenAI-compatible request types — Azure, Mistral define structurally identical message/tool/stream types. Share via `openai_compat` module | M | `adapters/src/{openai,azure,mistral}.rs` |
| LOW | Adapters | Remove error event wrapper functions in `convert.rs:105-128` — pure pass-throughs to `AssistantMessageEvent::error_*()` | S | `adapters/src/convert.rs` |
| LOW | Adapters | Have `proxy.rs` use shared `convert.rs` error event helpers instead of redefining | S | `adapters/src/proxy.rs:387-406` |

### Cross-Crate Duplication

| Severity | Area | Item | Effort | File(s) |
|----------|------|------|--------|---------|
| MEDIUM | Local-LLM | Message conversion reimplemented — should use core's `MessageConverter` trait or shared utility instead of standalone functions | M | `local-llm/src/convert.rs` vs `adapters/src/convert.rs` |
| LOW | Local-LLM | Remove duplicated test message builders (`user_msg`, `assistant_msg`, `tool_result_msg`) — same as `tests/common/mod.rs` | S | `local-llm/src/convert.rs:202-236` |
| LOW | Memory | Remove `unix_now()` wrapper — just import `swink_agent::now_timestamp()` directly | S | `memory/src/time.rs:23-25` |
| LOW | Core | Context sliding window logic duplicated between `context.rs:54-127` and `context_transformer.rs:99-171` (transformer adds reporting) | M | `src/context.rs`, `src/context_transformer.rs` |

---

## P3 — Consumer Experience

| Severity | Area | Item | Effort | File(s) |
|----------|------|------|--------|---------|
| HIGH | API Ergonomics | Add `AgentOptions::from_connections(prompt, connections)` — eliminate unintuitive `ModelConnections::into_parts()` decomposition | S | `src/agent.rs` |
| HIGH | API Ergonomics | Add `builtin_tools()` helper — returns `Vec<Arc<dyn AgentTool>>` with BashTool + ReadFileTool + WriteFileTool. Eliminates Arc wrapping boilerplate | S | `src/lib.rs` or `src/tools/mod.rs` |
| MEDIUM | API Ergonomics | Change TUI `launch()` to accept pre-built `AgentOptions` instead of closure — eliminates nested closure pattern | M | `tui/src/lib.rs` |
| MEDIUM | API Ergonomics | Simplify model preset access — add `preset("claude-sonnet-4-6")` convenience function vs deep `remote_preset_keys::anthropic::SONNET_46` path | S | `adapters/src/remote_presets.rs` |
| LOW | API Ergonomics | Auto-load `.env` inside `setup_terminal()` or `launch()` — remove copy-paste `dotenvy::dotenv().ok()` from examples | S | `tui/src/lib.rs` |

---

## P4 — Extensibility Roadmap

### Tier 2 — Unlocks Experimentation

| Severity | Area | Item | Effort |
|----------|------|------|--------|
| HIGH | Memory | State persistence / checkpointing abstraction — core has no awareness of persistence. No built-in checkpoint/resume. `ContextTransformer` is sync (can't fetch summaries) | M |
| HIGH | Memory | Context versioning / multi-layer memory — trait for retrieving old context snapshots, hooks for pre-computed summarization. Enables RAG, hierarchical context | M |
| MEDIUM | Lifecycle Events | Post-turn lifecycle hook — enables real-time memory persistence, metrics flush, or steering logic between turns | M |
| MEDIUM | Observability | Structured metrics/observability trait — per-tool timing, token usage breakdowns, cost attribution per turn | M |
| MEDIUM | Resilience | Model fallback mechanism — try a simpler/cheaper model on failure before exhausting retries | M |
| MEDIUM | Context | Pluggable token counting — replace the `chars/4` heuristic with a trait so callers can supply tiktoken or provider-native counts | M |
| MEDIUM | Messaging | Push-based messaging — complement poll-based `MessageProvider` so agents can listen on channels for external directives | S |
| MEDIUM | Lifecycle Events | Turn-level state snapshots in events — `TurnEnd` should carry full context at boundary for external replay/auditing | M |
| MEDIUM | Cost | Mid-turn cost/token gating — prevent single LLM call from exceeding budget. Currently `CostCapPolicy` only checks after turns | M |

### Tier 3 — Larger Initiatives

| Severity | Area | Item | Effort |
|----------|------|------|--------|
| HIGH | Multi-Agent | Orchestration layer — request/response channels between agents, agent hierarchies (parent/child), supervisor pattern. Currently registry + mailbox are barebones | L |
| HIGH | Durability | Loop pause/resume/checkpoint — serialize loop state for durable long-running workflows. Currently all-or-nothing | L |
| MEDIUM | Agent State | Agent config serialization — save/restore tool set, transformers, retry strategies, policies. Currently only messages persist | M |
| MEDIUM | Tool Execution | Tool execution ordering policy — sequential, priority, or DAG-based dispatch instead of always-concurrent | L |
| MEDIUM | Adapters | Per-provider feature flags — opt-in to thinking, vision, tool-use, etc. per model | M |
| LOW | Extensibility | CustomMessage serialization — `Box<dyn CustomMessage>` can't be serialized (no `dyn` serde). Breaks on store/load | M |
| LOW | Tool Discovery | Tool versioning / namespacing — group tools by version, namespace, or capability. Current flat discovery is sufficient | S |

---

## Completed

Previously planned items that have been implemented.

### Core Extensibility (Completed)

- [x] **LoopPolicy trait** — Custom stop conditions, max-turn limits, cost caps via `fn should_continue(&AgentResult) -> bool`
- [x] **SubAgent composition** — Agent-as-tool and `SubAgent` helper for hierarchical delegation and multi-agent workflows
- [x] **StreamMiddleware** — Intercept, log, filter, or transform LLM response events before accumulation
- [x] **ContextCompacted event** — Emits dropped messages so memory plugins can reactively trigger summarization
- [x] **ContextTransformer trait** — Replaced bare `TransformContextFn` closure with composable `trait ContextTransformer`
- [x] **SessionStoreAsync trait** — Async session storage for Redis, S3, and cloud storage backends
- [x] **Session metadata extensibility** — `custom: serde_json::Value` field on `SessionMeta` for plugin-defined tags
- [x] **SSE stream parser** — Shared `SseStreamParser` utility extracted from duplicated Anthropic/OpenAI code
- [x] **HTTP status classifier** — Default `HttpStatusClassifier` for HTTP status → `StreamErrorKind` mapping
- [x] **Provider config extensibility** — `provider_config: Option<serde_json::Value>` replacing provider-specific fields on `ModelSpec`
- [x] **ContentBlock::Extension** — Extension variant for multimodal plugins
- [x] **AgentError::Plugin** — Plugin error variant retaining semantic meaning
- [x] **Usage/Cost extra metrics** — `extra: HashMap<String, f64>` for provider-specific billing
- [x] **ToolApproval::ApprovedWith** — Approval can constrain scope or sanitize input
- [x] **BeforeLlmCall event** — Observe/log the final prompt after context transform
- [x] **TurnEnd reason enum** — Clarifies why a turn ended (no tools, steering interrupt, error)
- [x] **SessionStore filtering** — `list_filtered(&SessionFilter)` method
- [x] **ToolValidator trait** — Pre-execute hook for custom validators beyond JSON Schema
- [x] **Tool discovery methods** — `find_tool(name)` and `tools_matching(predicate)` on `Agent`
- [x] **Streaming on_update docs** — Documented callback pattern; used in `BashTool` for streaming stdout
- [x] **Adapter authoring example** — Template example + checklist for building new provider adapters

### Tier 1 Extensibility (Completed)

- [x] **Move ProxyStreamFn to adapters** — Removed `reqwest`/`eventsource-stream` from core dependency tree
- [x] **Feature-gate built-in tools** — `builtin-tools` feature (default-enabled) gates `BashTool`, `ReadFileTool`, `WriteFileTool`
- [x] **ToolCallTransformer trait** — Pre-execution argument rewriting hook
- [x] **Pluggable error retryability** — `RetryStrategy::should_retry()` is now the sole decision point

### TUI Features (Completed)

- [x] **Context Window Progress Bar** — Visual gauge showing context fill % with color transitions
- [x] **Collapsible Tool Result Blocks** — One-line summary with expand on Enter
- [x] **External Editor Mode** — `$EDITOR` integration for multi-line prompt composition
- [x] **Tiered Approval Modes** — Smart mode: auto-approve reads, prompt for writes
- [x] **Plan Mode** — Read-only mode restricting agent to read-only tools
- [x] **Inline Diff View** — Syntax-highlighted unified/side-by-side diffs with per-hunk approve/reject
