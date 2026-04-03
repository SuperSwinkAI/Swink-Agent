# Tasks: Plugin System

**Input**: Design documents from `/specs/037-plugin-system/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/

**Tests**: Included — the project constitution mandates test-driven development (tests before implementation).

**Organization**: Tasks grouped by user story for independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Phase 1: Setup

**Purpose**: Feature gate and module scaffolding

- [x] T001 Add `plugins` feature flag to `Cargo.toml` (not in default features)
- [x] T002 Create `src/plugin.rs` with `#[cfg(feature = "plugins")]` module and re-export in `src/lib.rs`
- [x] T003 Add `plugins` field (`Vec<Arc<dyn Plugin>>`) to `AgentOptions` in `src/agent_options.rs` behind `#[cfg(feature = "plugins")]`

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Plugin trait, PluginRegistry, and NamespacedTool — all user stories depend on these

**CRITICAL**: No user story work can begin until this phase is complete

- [x] T004 Define `Plugin` trait with all default methods (name, priority, on_init, policy methods, on_event, tools) in `src/plugin.rs`
- [x] T005 Write tests for `PluginRegistry` CRUD operations (register, unregister, get, list, is_empty, len) in `tests/plugin_registry.rs`
- [x] T006 Implement `PluginRegistry` struct (register with duplicate warn, unregister, get, list sorted by priority desc with stable sort) in `src/plugin.rs`
- [x] T007 Write tests for `NamespacedTool` (name returns `"{plugin}.{tool}"`, all other methods delegate) in `tests/plugin_registry.rs`
- [x] T008 Implement `NamespacedTool` struct that wraps `Arc<dyn AgentTool>` and overrides `name()` to return `"{plugin_name}.{tool_name}"` in `src/plugin.rs`

**Checkpoint**: Plugin trait, registry, and tool wrapper are complete and tested independently

---

## Phase 3: User Story 1 — Plugin Bundles Policies and Tools (Priority: P1) MVP

**Goal**: A consumer registers a single plugin and its policies appear in the correct slots, its tools are available, and its event observer fires.

**Independent Test**: Create a plugin contributing one post-turn policy and one tool, register it, run a conversation, verify the policy fires and the tool is in the tool list.

### Tests for User Story 1

- [ ] T009 [US1] Write test: plugin contributing a post-turn policy — verify policy evaluates during the loop in `tests/plugin_integration.rs`
- [ ] T010 [P] [US1] Write test: plugin contributing tools — verify tools appear namespaced in agent tool list in `tests/plugin_integration.rs`
- [ ] T011 [P] [US1] Write test: plugin with event observer — verify observer is called for AgentStart event in `tests/plugin_integration.rs`

### Implementation for User Story 1

- [ ] T012 [US1] Implement `with_plugin()` and `with_plugins()` builder methods on `AgentOptions` in `src/agent_options.rs`
- [ ] T013 [US1] Implement plugin contribution merge in `Agent::new()` — extract policies from each plugin (priority-sorted), prepend to policy vecs in `src/agent.rs`
- [ ] T014 [US1] Implement plugin tool extraction in `Agent::new()` — wrap each tool in `NamespacedTool`, append to tools vec in `src/agent.rs`
- [ ] T015 [US1] Implement plugin event observer integration in `Agent::new()` — convert `on_event` to `EventForwarderFn` closures, prepend to event_forwarders in `src/agent.rs`
- [ ] T016 [US1] Propagate merged policies and tools through `build_loop_config()` in `src/agent/invoke.rs` (verify existing flow handles merged vecs)

**Checkpoint**: A plugin's policies, tools, and event observer all function correctly. Single-plugin registration is the MVP.

---

## Phase 4: User Story 2 — Priority-Based Execution Order (Priority: P1)

**Goal**: Multiple plugins execute in priority order. Higher priority runs first, insertion order breaks ties.

**Independent Test**: Register two plugins with different priorities contributing pre-turn policies, verify higher-priority policy evaluates first.

### Tests for User Story 2

- [ ] T017 [US2] Write test: two plugins with different priorities — verify higher priority policy runs first in `tests/plugin_integration.rs`
- [ ] T018 [P] [US2] Write test: two plugins with same priority — verify insertion order preserved in `tests/plugin_integration.rs`
- [ ] T019 [P] [US2] Write test: higher-priority plugin returns Stop — verify lower-priority plugin's policy is not evaluated (short-circuit) in `tests/plugin_integration.rs`

### Implementation for User Story 2

- [ ] T020 [US2] Ensure `Agent::new()` sorts plugins by priority (descending, stable) before extracting contributions in `src/agent.rs`
- [ ] T021 [US2] Verify short-circuit semantics work across merged policy list (plugin + direct policies) — no changes to `src/policy.rs` expected, add test to confirm

**Checkpoint**: Multi-plugin priority ordering is deterministic and short-circuit works across the merged list.

---

## Phase 5: User Story 3 — Backward-Compatible Composition (Priority: P1)

**Goal**: Plugin policies run before directly-registered policies. Existing agents without plugins behave identically.

**Independent Test**: Configure agent with both a direct pre-turn policy and a plugin pre-turn policy, verify plugin's runs first.

### Tests for User Story 3

- [ ] T022 [US3] Write test: agent with direct policy and plugin policy — verify plugin policy runs first, then direct policy in `tests/plugin_integration.rs`
- [ ] T023 [P] [US3] Write test: agent with direct policies only (no plugins) — verify zero behavioral change in `tests/plugin_integration.rs`
- [ ] T024 [P] [US3] Write test: plugin Stop verdict prevents direct policies from evaluating in `tests/plugin_integration.rs`

### Implementation for User Story 3

- [ ] T025 [US3] Verify merge order in `Agent::new()`: plugin policies prepended, direct policies appended — adjust if needed in `src/agent.rs`

**Checkpoint**: Backward compatibility confirmed. Mixed plugin + direct policy configurations work correctly.

---

## Phase 6: User Story 4 — Registry Introspection (Priority: P2)

**Goal**: Consumers can list and look up registered plugins by name for debugging.

**Independent Test**: Register three plugins, query registry for names and priorities, look up by name.

### Tests for User Story 4

- [ ] T026 [US4] Write test: `agent.plugins()` returns all plugins in priority order in `tests/plugin_integration.rs`
- [ ] T027 [P] [US4] Write test: `agent.plugin("name")` returns correct plugin reference in `tests/plugin_integration.rs`
- [ ] T028 [P] [US4] Write test: `agent.plugin("nonexistent")` returns None in `tests/plugin_integration.rs`

### Implementation for User Story 4

- [ ] T029 [US4] Add `plugins: Vec<Arc<dyn Plugin>>` field to `Agent` struct (retained after merge for introspection) in `src/agent.rs`
- [ ] T030 [US4] Implement `plugins(&self) -> &[Arc<dyn Plugin>]` and `plugin(&self, name: &str) -> Option<&Arc<dyn Plugin>>` methods on Agent in `src/agent.rs`

**Checkpoint**: Plugin introspection API is functional.

---

## Phase 7: User Story 5 — Initialization Callback (Priority: P2)

**Goal**: Plugins receive `on_init(&self, &Agent)` during construction, in priority order, with panic safety.

**Independent Test**: Create a plugin with an init callback that records whether it was called, verify it fires once.

### Tests for User Story 5

- [ ] T031 [US5] Write test: plugin on_init is called once during Agent::new() in `tests/plugin_integration.rs`
- [ ] T032 [P] [US5] Write test: multiple plugins — on_init fires in priority order in `tests/plugin_integration.rs`
- [ ] T033 [P] [US5] Write test: panicking on_init is caught, agent construction continues, plugin policies still active in `tests/plugin_integration.rs`

### Implementation for User Story 5

- [ ] T034 [US5] Implement `on_init` dispatch loop in `Agent::new()` — iterate priority-sorted plugins, call `on_init(&self)` wrapped in `catch_unwind`, log panics via `tracing::warn!` in `src/agent.rs`

**Checkpoint**: Init callbacks fire correctly with panic safety.

---

## Phase 8: User Story 7 — Plugin Tool Contribution (Priority: P2)

**Goal**: Plugin-contributed tools are namespaced and merged with direct tools. Direct tools take precedence on collision.

**Independent Test**: Create a plugin contributing two tools, verify they appear namespaced in the agent's tool list.

### Tests for User Story 7

- [ ] T035 [US7] Write test: plugin tools appear as `"{plugin_name}.{tool_name}"` in agent tool list in `tests/plugin_integration.rs`
- [ ] T036 [P] [US7] Write test: two plugins contributing same-named tools — both appear with distinct namespace prefixes in `tests/plugin_integration.rs`
- [ ] T037 [P] [US7] Write test: direct tool with same name as namespaced plugin tool — direct tool found first when dispatching in `tests/plugin_integration.rs`

### Implementation for User Story 7

- [ ] T038 [US7] Verify tool merge order in `Agent::new()`: direct tools first, then namespaced plugin tools appended in `src/agent.rs`

**Checkpoint**: Plugin tools are correctly namespaced and merged.

---

## Phase 9: User Story 6 — Plugin Removal (Priority: P3)

**Goal**: Consumers can unregister a plugin by name before agent construction.

**Independent Test**: Register a plugin, unregister it, verify its policies and tools are absent.

### Tests for User Story 6

- [ ] T039 [US6] Write test: unregister plugin by name — verify its contributions are absent after Agent::new() in `tests/plugin_integration.rs`
- [ ] T040 [P] [US6] Write test: unregister nonexistent name — succeeds silently in `tests/plugin_registry.rs`

### Implementation for User Story 6

- [ ] T041 [US6] Verify `PluginRegistry::unregister()` removes plugin from the vec and subsequent `Agent::new()` does not include its contributions (should already work from Phase 2 registry + Phase 3 merge logic)

**Checkpoint**: Plugin removal works at configuration time.

---

## Phase 10: Polish & Cross-Cutting Concerns

**Purpose**: Documentation, test helpers, and feature gate verification

- [ ] T042 [P] Add `MockPlugin` test helper to `tests/common/mod.rs` (configurable name, priority, contributed policies/tools, init tracking)
- [ ] T043 [P] Verify `cargo test -p swink-agent --no-default-features` passes (plugins feature disabled, no compile errors)
- [ ] T044 [P] Verify `cargo test -p swink-agent --features plugins` passes (all plugin tests run)
- [ ] T045 [P] Verify `cargo clippy --workspace -- -D warnings` passes with plugins feature enabled
- [ ] T046 Update `src/lib.rs` public API re-exports: `Plugin`, `PluginRegistry` behind `#[cfg(feature = "plugins")]`
- [ ] T047 Add plugin system entry to CLAUDE.md lessons learned section

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup — BLOCKS all user stories
- **US1 (Phase 3)**: Depends on Foundational — MVP target
- **US2 (Phase 4)**: Depends on US1 (extends merge logic)
- **US3 (Phase 5)**: Depends on US1 (validates composition)
- **US4 (Phase 6)**: Depends on US1 (needs plugins stored on Agent)
- **US5 (Phase 7)**: Depends on US1 (needs Agent::new() integration point)
- **US7 (Phase 8)**: Depends on US1 (extends tool merge)
- **US6 (Phase 9)**: Depends on Foundational (registry-level, no Agent needed)
- **Polish (Phase 10)**: Depends on all user stories complete

### User Story Independence

- **US1**: Core — must complete first (MVP)
- **US2, US3, US4, US5, US7**: Can proceed in parallel after US1
- **US6**: Can proceed after Phase 2 (independent of US1)

### Parallel Opportunities

- T005 + T007: Registry and NamespacedTool tests (different test scenarios)
- T010 + T011: Tool and event observer tests (different aspects)
- T018 + T019: Priority tie and short-circuit tests
- T023 + T024: No-plugin and Stop-verdict tests
- T027 + T028: Lookup and missing lookup tests
- T032 + T033: Init order and panic tests
- T036 + T037: Collision and precedence tests
- T042-T045: Polish tasks (all independent)

---

## Parallel Example: User Story 1

```bash
# Launch US1 tests together (after foundational phase):
Task: "T010 [P] [US1] Write test: plugin contributing tools"
Task: "T011 [P] [US1] Write test: plugin with event observer"

# Then implement sequentially:
Task: "T012 [US1] Implement with_plugin() builder"
Task: "T013 [US1] Implement policy merge in Agent::new()"
Task: "T014 [US1] Implement tool extraction in Agent::new()"
Task: "T015 [US1] Implement event observer integration"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (feature gate + module scaffold)
2. Complete Phase 2: Foundational (trait + registry + NamespacedTool)
3. Complete Phase 3: User Story 1 (single plugin registration works end-to-end)
4. **STOP and VALIDATE**: `cargo test -p swink-agent --features plugins`
5. A consumer can now register one plugin and see its contributions active

### Incremental Delivery

1. Setup + Foundational → Plugin infrastructure ready
2. Add US1 → Single plugin works (MVP!)
3. Add US2 → Multi-plugin priority ordering works
4. Add US3 → Backward compatibility confirmed
5. Add US4 → Introspection API available
6. Add US5 → Init callbacks fire
7. Add US7 → Tool namespacing verified
8. Add US6 → Plugin removal works
9. Each story adds value without breaking previous stories

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- All tests follow TDD: write test first, ensure it fails, then implement
- Commit after each task or logical group
- The plugin module is entirely behind `#[cfg(feature = "plugins")]` — zero cost when disabled
- No changes to `src/policy.rs` or `src/loop_/` — plugin contributions flow through existing infrastructure
