# Swink-Agent Specification Tracker

**Related Documents:**
- Product Requirements: [PRD.md](PRD.md) — §1–§17
- Architecture: [../architecture/HLD.md](../architecture/HLD.md) — Component diagram, data flow, dependency graph
- Constitution: [../../.specify/memory/constitution.md](../../.specify/memory/constitution.md)
- Provider Roadmap: [PROVIDER_EXPANSION_ROADMAP.md](PROVIDER_EXPANSION_ROADMAP.md)
- Eval Roadmap: [EVAL.md](EVAL.md)

**Current Focus:** 20/30 specs have plans, 18/30 have tasks, 15/30 complete (001–014, 020). Phase 0 + Phase 1 + Phase 2 done. Phase 3 partially done (011–014, 020 complete). Next: `/speckit.implement` for 021-memory-crate and 022-local-llm-crate, `/speckit.tasks` for 028 and 030. 10 specs need plans (015–019, 023–025, 027, 029).

> **Numbering System:** Spec numbers (001–030) are sequential identifiers that
> never change. Phase numbers represent execution order and can be reassigned
> as priorities shift.

---

## Phase 0: Foundation

**Goal:** Establish the workspace scaffold and core type system that every
other crate and module depends on — the data model, error taxonomy, and
pluggable trait boundaries.

**Status:** 3/3 specs planned, 3/3 have tasks, 3/3 complete, 3/3 specs defined

### Implementation Checklist

- [x] **0.1** Workspace & Cargo Scaffold — 7-crate workspace, centralized deps, feature flags, MSRV/edition, toolchain config (§15)
  - Spec: `specs/001-workspace-scaffold/spec.md`
  - Branch: `001-workspace-scaffold`
  - Status: Complete (24/24 tasks, merged to main)
  - Depends on: —
- [x] **0.2** Foundation Types & Errors — ContentBlock, LlmMessage, AgentMessage, Usage, Cost, StopReason, ModelSpec, AgentError (§3, §10.3)
  - Spec: `specs/002-foundation-types-errors/spec.md`
  - Branch: `002-foundation-types-errors`
  - Status: Complete (63/63 tasks, merged to main)
  - Depends on: 0.1
- [x] **0.3** Core Traits — AgentTool, StreamFn, RetryStrategy, JSON Schema validation, delta accumulation (§4, §7, §11)
  - Spec: `specs/003-core-traits/spec.md`
  - Branch: `003-core-traits`
  - Status: Complete (47/47 tasks, merged to main)
  - Depends on: 0.2

---

## Phase 1: Core Engine

**Goal:** The agent loop execution engine and its public API wrapper —
the two central verticals that make the agent functional.

**Status:** 2/2 specs planned, 2/2 have tasks, 2/2 complete, 2/2 specs defined

### Implementation Checklist

- [x] **1.1** Agent Loop — Nested inner/outer loop, concurrent tool dispatch, steering/follow-up, retry, overflow recovery (§8, §9, §12)
  - Spec: `specs/004-agent-loop/spec.md`
  - Branch: `004-agent-loop`
  - Status: Complete (65/65 tasks, merged to main)
  - Depends on: 0.2, 0.3
- [x] **1.2** Agent Struct & Public API — Stateful wrapper, streaming/async/sync API, structured output, queues, subscriptions (§13)
  - Spec: `specs/005-agent-struct/spec.md`
  - Branch: `005-agent-struct`
  - Status: Complete (78/78 tasks, merged to main)
  - Depends on: 1.1

---

## Phase 2: Core Extensions

**Goal:** Context management, tool system extensions, model catalog, multi-agent
primitives, and loop governance — capabilities that enhance the core engine.

**Status:** 5/5 specs planned, 5/5 have tasks, 5/5 complete, 5/5 specs defined

### Implementation Checklist

- [x] **2.1** Context Management — Sliding window pruning, transform hooks (sync/async), versioned history, convert_to_llm pipeline (§5, §10.1)
  - Spec: `specs/006-context-management/spec.md`
  - Branch: `006-context-management`
  - Status: Complete (46/46 tasks, merged to main)
  - Depends on: 0.2
- [x] **2.2** Tool System Extensions — Transformer, validator, middleware, execution policies, FnTool, builtin tools (§4)
  - Spec: `specs/007-tool-system-extensions/spec.md`
  - Branch: `007-tool-system-extensions`
  - Status: Complete (64/64 tasks, merged to main)
  - Depends on: 0.3
- [x] **2.3** Model Catalog, Presets & Fallback — TOML-driven catalog, preset-to-connection resolution, automatic fallback chain
  - Spec: `specs/008-model-catalog-presets/spec.md`
  - Branch: `008-model-catalog-presets`
  - Status: Complete (30/30 tasks, merged to main)
  - Depends on: 0.3
- [x] **2.4** Multi-Agent System — AgentRegistry, AgentMailbox, AgentOrchestrator, SubAgent tool wrapper
  - Spec: `specs/009-multi-agent-system/spec.md`
  - Branch: `009-multi-agent-system`
  - Status: Complete (59/59 tasks, merged to main)
  - Depends on: 1.2
- [x] **2.5** Loop Policies & Observability — LoopPolicy, StreamMiddleware, MetricsCollector, PostTurnHook, BudgetGuard, Checkpoint
  - Spec: `specs/010-loop-policies-observability/spec.md`
  - Branch: `010-loop-policies-observability`
  - Status: Complete (72/72 tasks, merged to main)
  - Depends on: 1.1

---

## Phase 3: Adapters

**Goal:** LLM provider adapters — shared infrastructure and one adapter per
provider. Each adapter implements StreamFn for its provider's streaming protocol.

**Status:** 5/10 specs planned, 5/10 have tasks, 5/10 complete, 10/10 specs defined

### Implementation Checklist

- [x] **3.1** Adapter Shared Infrastructure — MessageConverter trait, HttpErrorClassifier, SSE parsing, remote preset construction (§15.1)
  - Spec: `specs/011-adapter-shared-infra/spec.md`
  - Branch: `011-adapter-shared-infra`
  - Status: Complete (40/40 tasks, merged to main)
  - Depends on: 0.3
- [x] **3.2** Adapter: Anthropic — AnthropicStreamFn, /v1/messages SSE, thinking blocks with budget control (§15.1)
  - Spec: `specs/012-adapter-anthropic/spec.md`
  - Branch: `012-adapter-anthropic`
  - Status: Complete (73/73 tasks, merged to main)
  - Depends on: 3.1
- [x] **3.3** Adapter: OpenAI — OpenAiStreamFn, /v1/chat/completions SSE, multi-provider compatible (§15.1)
  - Spec: `specs/013-adapter-openai/spec.md`
  - Branch: `013-adapter-openai`
  - Status: Complete (73/73 tasks, merged to main)
  - Depends on: 3.1
- [x] **3.4** Adapter: Ollama — OllamaStreamFn, /api/chat NDJSON, native tool-calling (§15.1)
  - Spec: `specs/014-adapter-ollama/spec.md`
  - Branch: `014-adapter-ollama`
  - Status: Complete (74/74 tasks, merged to main)
  - Depends on: 3.1
- [ ] **3.5** Adapter: Google Gemini — GeminiStreamFn, Gemini API SSE (§15.1)
  - Spec: `specs/015-adapter-gemini/spec.md`
  - Branch: `015-adapter-gemini`
  - Status: Specify complete — needs plan + tasks
  - Depends on: 3.1
- [ ] **3.6** Adapter: Azure OpenAI — AzureStreamFn, deployment routing, API versioning (§15.1)
  - Spec: `specs/016-adapter-azure/spec.md`
  - Branch: `016-adapter-azure`
  - Status: Specify complete — needs plan + tasks
  - Depends on: 3.1
- [ ] **3.7** Adapter: xAI — XAiStreamFn, xAI (Grok) SSE (§15.1)
  - Spec: `specs/017-adapter-xai/spec.md`
  - Branch: `017-adapter-xai`
  - Status: Specify complete — needs plan + tasks
  - Depends on: 3.1
- [ ] **3.8** Adapter: Mistral — MistralStreamFn, Mistral API SSE (§15.1)
  - Spec: `specs/018-adapter-mistral/spec.md`
  - Branch: `018-adapter-mistral`
  - Status: Specify complete — needs plan + tasks
  - Depends on: 3.1
- [ ] **3.9** Adapter: AWS Bedrock — BedrockStreamFn, SSE, AWS SigV4 signing (§15.1)
  - Spec: `specs/019-adapter-bedrock/spec.md`
  - Branch: `019-adapter-bedrock`
  - Status: Specify complete — needs plan + tasks
  - Depends on: 3.1
- [x] **3.10** Adapter: Proxy — ProxyStreamFn, SSE, bearer auth, typed delta events (§7.4, §15.1)
  - Spec: `specs/020-adapter-proxy/spec.md`
  - Branch: `020-adapter-proxy`
  - Status: Complete (40/40 tasks, merged to main)
  - Depends on: 3.1

### Parallel Opportunities

After **3.1 Shared Infrastructure** completes, all 9 provider adapters (3.2–3.10) can proceed in parallel — they are independent implementations of the same trait.

---

## Phase 4: Companion Crates

**Goal:** Standalone crates for session persistence, on-device inference, and
evaluation — each depends only on the core library.

**Status:** 2/4 specs planned, 2/4 have tasks, 4/4 specs defined

### Implementation Checklist

- [ ] **4.1** Memory Crate — SessionStore (sync/async), JsonlSessionStore, SummarizingCompactor, session metadata
  - Spec: `specs/021-memory-crate/spec.md`
  - Branch: `021-memory-crate`
  - Status: Ready for implementation (0/57 tasks)
  - Depends on: 0.2
- [ ] **4.2** Local LLM Crate — LocalModel (SmolLM3-3B), LocalStreamFn, EmbeddingModel, presets, progress reporting
  - Spec: `specs/022-local-llm-crate/spec.md`
  - Branch: `022-local-llm-crate`
  - Status: Ready for implementation (0/58 tasks)
  - Depends on: 0.3
- [ ] **4.3** Eval: Trajectory & Matching — TrajectoryCollector, TrajectoryMatcher, EfficiencyEvaluator, ResponseCriteria
  - Spec: `specs/023-eval-trajectory-matching/spec.md`
  - Branch: `023-eval-trajectory-matching`
  - Status: Specify complete — needs plan + tasks
  - Depends on: 1.1
- [ ] **4.4** Eval: Runner, Scoring & Governance — EvalRunner, EvaluatorRegistry, FsEvalStore, CI/CD gating, audit trails
  - Spec: `specs/024-eval-runner-governance/spec.md`
  - Branch: `024-eval-runner-governance`
  - Status: Specify complete — needs plan + tasks
  - Depends on: 4.3

---

## Phase 5: Terminal UI

**Goal:** Interactive terminal interface — the binary crate that demonstrates
the full agent library in a usable application.

**Status:** 2/5 specs planned, 1/5 have tasks, 5/5 specs defined

### Implementation Checklist

- [ ] **5.1** TUI: Scaffold, Event Loop & Config — Binary entry, terminal setup, async event loop, config, credentials, wizard (§16.1–16.2)
  - Spec: `specs/025-tui-scaffold-config/spec.md`
  - Branch: `025-tui-scaffold-config`
  - Status: Specify complete — needs plan + tasks
  - Depends on: 1.2, 3.1
- [ ] **5.2** TUI: Input & Conversation — Multi-line editor, scrollable conversation, markdown, syntax highlighting (§16.2–16.3)
  - Spec: `specs/026-tui-input-conversation/spec.md`
  - Branch: `026-tui-input-conversation`
  - Status: Ready for implementation (0/69 tasks)
  - Depends on: 5.1
- [ ] **5.3** TUI: Tool Panel, Diffs & Status Bar — Tool panel, collapsible blocks, inline diffs, status bar, context gauge (§16.6–16.7, §16.10)
  - Spec: `specs/027-tui-tools-diffs-status/spec.md`
  - Branch: `027-tui-tools-diffs-status`
  - Status: Specify complete — needs plan + tasks
  - Depends on: 5.2
- [ ] **5.4** TUI: Commands, Editor & Session — Hash/slash commands, external editor, session persistence (§16.4, §16.8)
  - Spec: `specs/028-tui-commands-editor-session/spec.md`
  - Branch: `028-tui-commands-editor-session`
  - Status: Plan complete — needs tasks
  - Depends on: 5.2
- [ ] **5.5** TUI: Plan Mode & Approval — Plan mode (read-only restriction), tiered approval (Enabled/Smart/Bypassed), session trust (§16.9, §16.11)
  - Spec: `specs/029-tui-plan-mode-approval/spec.md`
  - Branch: `029-tui-plan-mode-approval`
  - Status: Specify complete — needs plan + tasks
  - Depends on: 5.2

### Parallel Opportunities

After **5.2 Input & Conversation** completes, specs 5.3, 5.4, and 5.5 can proceed in parallel — they are independent feature layers on top of the conversation UI.

---

## Phase 6: Integration Testing

**Goal:** End-to-end tests exercising the full stack against all PRD acceptance
criteria.

**Status:** 1/1 specs planned, 0/1 have tasks, 1/1 specs defined

### Implementation Checklist

- [ ] **6.1** Integration Tests — MockStreamFn, MockTool, EventCollector, tests for all 30 PRD acceptance criteria (§17)
  - Spec: `specs/030-integration-tests/spec.md`
  - Branch: `030-integration-tests`
  - Status: Plan complete — needs tasks
  - Depends on: 1.2, 2.1, 2.2

---

## Dependencies

```mermaid
graph TD
    subgraph Phase 0: Foundation
        A["0.1 Workspace Scaffold"]
        B["0.2 Foundation Types"]
        C["0.3 Core Traits"]
    end

    subgraph Phase 1: Core Engine
        D["1.1 Agent Loop"]
        E["1.2 Agent Struct"]
    end

    subgraph Phase 2: Core Extensions
        F["2.1 Context Management"]
        G["2.2 Tool Extensions"]
        H["2.3 Model Catalog"]
        I["2.4 Multi-Agent"]
        J["2.5 Policies & Observability"]
    end

    subgraph Phase 3: Adapters
        K["3.1 Shared Infra"]
        L["3.2–3.10 Provider Adapters"]
    end

    subgraph Phase 4: Companion Crates
        M["4.1 Memory"]
        N["4.2 Local LLM"]
        O["4.3 Eval: Trajectory"]
        P["4.4 Eval: Runner"]
    end

    subgraph Phase 5: Terminal UI
        Q["5.1 TUI Scaffold"]
        R["5.2 Input & Conversation"]
        S["5.3 Tools & Diffs"]
        T["5.4 Commands & Editor"]
        U["5.5 Plan Mode & Approval"]
    end

    subgraph Phase 6: Integration
        V["6.1 Integration Tests"]
    end

    A --> B
    B --> C
    B --> F
    B --> M
    C --> D
    C --> G
    C --> H
    C --> K
    C --> N
    D --> E
    D --> J
    D --> O
    E --> I
    E --> Q
    K --> L
    K --> Q
    O --> P
    Q --> R
    R --> S
    R --> T
    R --> U
    E --> V
    F --> V
    G --> V

    style A fill:#22c55e,color:#000,stroke:#16a34a,stroke-width:2px
    style B fill:#22c55e,color:#000,stroke:#16a34a,stroke-width:2px
    style C fill:#22c55e,color:#000,stroke:#16a34a,stroke-width:2px
    style D fill:#22c55e,color:#000,stroke:#16a34a,stroke-width:2px
    style E fill:#22c55e,color:#000,stroke:#16a34a,stroke-width:2px
    style F fill:#22c55e,color:#000,stroke:#16a34a,stroke-width:2px
    style G fill:#22c55e,color:#000,stroke:#16a34a,stroke-width:2px
    style H fill:#22c55e,color:#000,stroke:#16a34a,stroke-width:2px
    style I fill:#22c55e,color:#000,stroke:#16a34a,stroke-width:2px
    style J fill:#22c55e,color:#000,stroke:#16a34a,stroke-width:2px
    style K fill:#22c55e,color:#000,stroke:#16a34a,stroke-width:2px
    style L fill:#fff,color:#000,stroke:#6b7280,stroke-width:2px
    style M fill:#fff,color:#000,stroke:#6b7280,stroke-width:2px
    style N fill:#fff,color:#000,stroke:#6b7280,stroke-width:2px
    style O fill:#fff,color:#000,stroke:#6b7280,stroke-width:2px
    style P fill:#fff,color:#000,stroke:#6b7280,stroke-width:2px
    style Q fill:#fff,color:#000,stroke:#6b7280,stroke-width:2px
    style R fill:#fff,color:#000,stroke:#6b7280,stroke-width:2px
    style S fill:#fff,color:#000,stroke:#6b7280,stroke-width:2px
    style T fill:#fff,color:#000,stroke:#6b7280,stroke-width:2px
    style U fill:#fff,color:#000,stroke:#6b7280,stroke-width:2px
    style V fill:#fff,color:#000,stroke:#6b7280,stroke-width:2px
```

> ⬜ Not started · 🟢 Complete · 🟡 In progress · 🔴 Blocked

### Critical Path

```
0.1 Scaffold → 0.2 Types → 0.3 Traits → 1.1 Loop → 1.2 Agent Struct → 5.1 TUI Scaffold → 5.2 Input (full app)
                                       ↘ 3.1 Shared Infra → 3.2–3.10 Adapters (provider coverage)
                           ↘ 2.1 Context, 2.2 Tools, 2.3 Catalog (extensions)
                                       ↘ 4.3 Eval Trajectory → 4.4 Eval Runner (evaluation)
```

### Parallel Opportunities

After **0.3 Core Traits** completes, the following can proceed in parallel:
- **Track A (Engine):** 1.1 Agent Loop → 1.2 Agent Struct
- **Track B (Adapters):** 3.1 Shared Infra → 3.2–3.10 (all 9 in parallel)
- **Track C (Extensions):** 2.1 Context, 2.2 Tool Extensions, 2.3 Model Catalog

After **1.2 Agent Struct** completes:
- **Track D (Multi-Agent):** 2.4 Multi-Agent System
- **Track E (TUI):** 5.1 Scaffold → 5.2 Input → 5.3/5.4/5.5 (three in parallel)

After **0.2 Foundation Types** completes:
- **Track F (Companion):** 4.1 Memory (depends only on core types)
