# TODO

## Extensibility Roadmap

Improvements to make swink-agent a robust, extensible core for agentic research. Grouped by priority tier.

### Tier 2 — Unlocks Experimentation

| # | Area | Item | Effort |
|---|------|------|--------|
| 5 | Lifecycle Events | Post-turn lifecycle hook — enables real-time memory persistence, metrics flush, or steering logic between turns | M |
| 6 | Observability | Structured metrics/observability trait — per-tool timing, token usage breakdowns, cost attribution per turn | M |
| 7 | Resilience | Model fallback mechanism — try a simpler/cheaper model on failure before exhausting retries | M |
| 8 | Context | Pluggable token counting — replace the `chars/4` heuristic with a trait so callers can supply tiktoken or provider-native counts | M |

### Tier 3 — Larger Initiatives

| # | Area | Item | Effort |
|---|------|------|--------|
| 9 | Tool Execution | Tool execution ordering policy — sequential, priority, or DAG-based dispatch instead of always-concurrent | L |
| 10 | Durability | Pause/resume/checkpoint capability — serialize loop state for durable long-running workflows | L |
| 11 | Adapters | Per-provider feature flags in adapters crate — opt-in to thinking, vision, tool-use, etc. per model | M |

**Effort key:** S = small (< 1 day), M = medium (1–3 days), L = large (3+ days)

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

- [x] **Move ProxyStreamFn to adapters** — Removed `reqwest`/`eventsource-stream` from core dependency tree. Breaking: `swink_agent::ProxyStreamFn` → `swink_agent_adapters::ProxyStreamFn`
- [x] **Feature-gate built-in tools** — `builtin-tools` feature (default-enabled) gates `BashTool`, `ReadFileTool`, `WriteFileTool`. Disable with `default-features = false`
- [x] **ToolCallTransformer trait** — Pre-execution argument rewriting hook. Runs after approval, before validation. Blanket closure impl. Builder: `AgentOptions::with_tool_call_transformer()`
- [x] **Pluggable error retryability** — `RetryStrategy::should_retry()` is now the sole decision point in the loop (removed `is_retryable()` pre-check). Custom strategies can retry any error variant

### TUI Features (Completed)

- [x] **Context Window Progress Bar** — Visual gauge showing context fill % with color transitions
- [x] **Collapsible Tool Result Blocks** — One-line summary with expand on Enter
- [x] **External Editor Mode** — `$EDITOR` integration for multi-line prompt composition
- [x] **Tiered Approval Modes** — Smart mode: auto-approve reads, prompt for writes
- [x] **Plan Mode** — Read-only mode restricting agent to read-only tools
- [x] **Inline Diff View** — Syntax-highlighted unified/side-by-side diffs with per-hunk approve/reject
