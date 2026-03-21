# Tasks: Tool System Extensions

**Input**: Design documents from `/specs/007-tool-system-extensions/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/public-api.md

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

**TDD Note**: Per constitution principle II (Test-Driven Development), test tasks within each phase MUST be executed before their corresponding implementation tasks, regardless of task ID ordering. Write tests first, verify they fail, then implement.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Feature flag configuration and shared type preparation

- [ ] T001 Add `builtin-tools` feature flag (default-enabled) to `Cargo.toml` with `tokio/process` dep gated behind it
- [ ] T002 [P] Add `regex` workspace dependency to root `Cargo.toml` for sensitive value pattern matching
- [ ] T003 [P] Create `src/tools/` directory with `src/tools/mod.rs` for feature-gated built-in tool re-exports

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core types and validation functions in `src/tool.rs` that ALL user stories depend on

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [ ] T004 Implement `ToolMetadata` struct with `namespace`/`version` fields and `with_namespace()`/`with_version()` builders in `src/tool.rs`
- [ ] T005 Implement `ToolApproval` enum (`Approved`, `Rejected`, `ApprovedWith(Value)`) in `src/tool.rs`
- [ ] T006 Implement `ToolApprovalRequest` struct with redacted `Debug` impl in `src/tool.rs`
- [ ] T007 Implement `ApprovalMode` enum (`Enabled`, `Smart`, `Bypassed`) with `Default` derive in `src/tool.rs`
- [ ] T008 Implement `validate_schema()` and `validate_tool_arguments()` functions using `jsonschema` crate in `src/tool.rs`
- [ ] T009 [P] Implement `unknown_tool_result()` and `validation_error_result()` helper constructors in `src/tool.rs`
- [ ] T010 [P] Implement `redact_sensitive_values()` with regex-based pattern matching for keys (password, secret, token, api_key) and value prefixes (sk-, key-, bearer) in `src/tool.rs`
- [ ] T011 Implement `selective_approve()` helper that wraps an approval callback with `ApprovalMode` filtering in `src/tool.rs`
- [ ] T012 Add `AgentTool` trait methods `requires_approval()` (default false) and `metadata()` (default None) to `src/tool.rs`

**Checkpoint**: Foundation ready — tool system types available for all user stories

---

## Phase 3: User Story 1 — Rewrite Tool Calls Before Execution (Priority: P1) 🎯 MVP

**Goal**: Enable developers to register a transformer that rewrites tool call arguments before validation and execution

**Independent Test**: Register a transformer that modifies an argument, invoke a tool, verify the tool receives modified arguments

### Implementation for User Story 1

- [ ] T013 [US1] Define `ToolCallTransformer` trait with `transform(&self, tool_name: &str, arguments: &mut Value)` method in `src/tool_call_transformer.rs`
- [ ] T014 [US1] Implement blanket impl of `ToolCallTransformer` for `Fn(&str, &mut Value) + Send + Sync` closures in `src/tool_call_transformer.rs`
- [ ] T015 [US1] Add `tool_call_transformer: Option<Arc<dyn ToolCallTransformer>>` field to `AgentLoopConfig` in `src/loop_/mod.rs`
- [ ] T016 [US1] Integrate transformer into tool dispatch pipeline (after approval, before validator) in `src/loop_/tool_dispatch.rs`
- [ ] T017 [US1] Add unit tests: transformer modifies args, no transformer passes through, closure blanket impl works in `src/tool_call_transformer.rs`
- [ ] T018 [US1] Re-export `ToolCallTransformer` from `src/lib.rs`

**Checkpoint**: Transformer pipeline functional — arguments can be rewritten before validation

---

## Phase 4: User Story 2 — Validate Tool Calls Before Execution (Priority: P1)

**Goal**: Enable developers to register a validator that accepts or rejects tool calls after transformation

**Independent Test**: Register a validator that rejects a specific tool name, verify the tool is not executed

### Implementation for User Story 2

- [ ] T019 [US2] Define `ToolValidator` trait with `validate(&self, tool_name: &str, arguments: &Value) -> Result<(), String>` method in `src/tool_validator.rs`
- [ ] T020 [US2] Implement blanket impl of `ToolValidator` for `Fn(&str, &Value) -> Result<(), String> + Send + Sync` closures in `src/tool_validator.rs`
- [ ] T021 [US2] Add `tool_validator: Option<Arc<dyn ToolValidator>>` field to `AgentLoopConfig` in `src/loop_/mod.rs`
- [ ] T022 [US2] Integrate validator into tool dispatch pipeline (after transformer, before schema validation) in `src/loop_/tool_dispatch.rs`
- [ ] T023 [US2] Add unit tests: validator accepts, validator rejects with error result, closure blanket impl works in `src/tool_validator.rs`
- [ ] T024 [US2] Re-export `ToolValidator` from `src/lib.rs`

**Checkpoint**: Validator pipeline functional — tool calls can be rejected before execution

---

## Phase 5: User Story 3 — Wrap Tool Execution with Middleware (Priority: P2)

**Goal**: Enable developers to wrap a tool's execute function with composable middleware for cross-cutting concerns

**Independent Test**: Wrap a tool with logging middleware, verify middleware runs around the tool's execute call

### Implementation for User Story 3

- [ ] T025 [US3] Define `MiddlewareFn` type alias for the middleware closure signature in `src/tool_middleware.rs`
- [ ] T026 [US3] Implement `ToolMiddleware` struct with `inner: Arc<dyn AgentTool>` and `middleware_fn: Arc<MiddlewareFn>` fields in `src/tool_middleware.rs`
- [ ] T027 [US3] Implement `ToolMiddleware::new()` constructor accepting inner tool and closure in `src/tool_middleware.rs`
- [ ] T028 [US3] Implement `AgentTool` for `ToolMiddleware` — delegate metadata methods to inner, intercept `execute()` in `src/tool_middleware.rs`
- [ ] T029 [P] [US3] Implement `ToolMiddleware::with_timeout()` factory method using `tokio::time::timeout` in `src/tool_middleware.rs`
- [ ] T030 [P] [US3] Implement `ToolMiddleware::with_logging()` factory method calling callback with (name, id, is_start) in `src/tool_middleware.rs`
- [ ] T031 [US3] Add unit tests: middleware intercepts execute, metadata delegates to inner, with_timeout enforces limit, with_logging calls callback in `src/tool_middleware.rs`
- [ ] T032 [US3] Re-export `ToolMiddleware` from `src/lib.rs`

**Checkpoint**: Middleware functional — tools can be decorated with composable wrappers

---

## Phase 6: User Story 4 — Control Tool Execution Order (Priority: P2)

**Goal**: Enable developers to configure execution policy for batched tool calls (concurrent, sequential, or priority-based)

**Independent Test**: Configure sequential policy, verify tools execute one after another rather than concurrently

### Implementation for User Story 4

- [ ] T033 [US4] Define `ToolCallSummary<'a>` borrowed view struct with `id`, `name`, `arguments` fields in `src/tool_execution_policy.rs`
- [ ] T034 [US4] Define `PriorityFn` type alias (`dyn Fn(&ToolCallSummary) -> i32 + Send + Sync`) in `src/tool_execution_policy.rs`
- [ ] T035 [US4] Define `ToolExecutionStrategy` trait with async `partition()` method returning `Vec<Vec<usize>>` in `src/tool_execution_policy.rs`
- [ ] T036 [US4] Implement `ToolExecutionPolicy` enum with `Concurrent`, `Sequential`, `Priority`, `Custom` variants in `src/tool_execution_policy.rs`
- [ ] T037 [US4] Implement `Clone` and `Debug` manually for `ToolExecutionPolicy` (Arc fields require manual impls) in `src/tool_execution_policy.rs`
- [ ] T038 [US4] Add `tool_execution_policy: ToolExecutionPolicy` field to `AgentLoopConfig` in `src/loop_/mod.rs`
- [ ] T039 [US4] Integrate execution policy into tool dispatch — compute groups from policy, execute groups sequentially with concurrent within-group in `src/loop_/tool_dispatch.rs`
- [ ] T040 [US4] Add unit tests: concurrent runs all at once, sequential runs in order, priority groups by value, custom strategy partitions in `src/tool_execution_policy.rs`
- [ ] T041 [US4] Re-export `ToolCallSummary`, `PriorityFn`, `ToolExecutionPolicy`, `ToolExecutionStrategy` from `src/lib.rs`

**Checkpoint**: Execution policies functional — tool dispatch order is configurable

---

## Phase 7: User Story 5 — Create Tools from Closures (Priority: P2)

**Goal**: Enable developers to create tools from closures without defining a full struct

**Independent Test**: Create a closure-based tool, register it, verify it executes correctly when called

### Implementation for User Story 5

- [ ] T042 [US5] Define `ExecuteFn` type alias for the stored execution closure in `src/fn_tool.rs`
- [ ] T043 [US5] Implement `FnTool` struct with `name`, `label`, `description`, `schema`, `requires_approval`, `execute_fn` fields in `src/fn_tool.rs`
- [ ] T044 [US5] Implement `FnTool::new()` constructor with default schema (accepts any object) and stub execute in `src/fn_tool.rs`
- [ ] T045 [US5] Implement builder methods `with_schema_for::<T: JsonSchema>()`, `with_schema(Value)`, `with_requires_approval(bool)` in `src/fn_tool.rs`
- [ ] T046 [US5] Implement `with_execute()` (full signature) and `with_execute_simple()` (params + cancel only) builder methods in `src/fn_tool.rs`
- [ ] T047 [US5] Implement `AgentTool` for `FnTool` — delegate to stored fields and closure in `src/fn_tool.rs`
- [ ] T048 [US5] Implement `Debug` for `FnTool` (closure fields use opaque display) in `src/fn_tool.rs`
- [ ] T049 [US5] Add unit tests: FnTool executes closure, with_schema_for derives schema, with_execute_simple works, trait methods delegate correctly in `src/fn_tool.rs`
- [ ] T050 [US5] Re-export `FnTool` from `src/lib.rs`

**Checkpoint**: FnTool functional — tools can be created from closures with zero boilerplate

---

## Phase 8: User Story 6 — Use Built-In Shell and File Tools (Priority: P3)

**Goal**: Provide pre-made tools for shell execution, file reading, and file writing behind a feature gate

**Independent Test**: Enable the feature flag, register built-in tools, verify they execute correctly

### Implementation for User Story 6

- [ ] T051 [US6] Implement `BashTool` struct with pre-computed JSON Schema, `new()`/`Default`, and `AgentTool` impl (name: "bash", requires_approval: true) using `tokio::process::Command` in `src/tools/bash.rs`
- [ ] T052 [P] [US6] Implement `ReadFileTool` struct with JSON Schema, `new()`/`Default`, and `AgentTool` impl (name: "read_file", requires_approval: false) using `tokio::fs::read_to_string` in `src/tools/read_file.rs`
- [ ] T053 [P] [US6] Implement `WriteFileTool` struct with JSON Schema, `new()`/`Default`, and `AgentTool` impl (name: "write_file", requires_approval: true) using `tokio::fs::write` in `src/tools/write_file.rs`
- [ ] T054 [US6] Implement `builtin_tools()` convenience function returning `Vec<Arc<dyn AgentTool>>` with all three tools in `src/tools/mod.rs`
- [ ] T055 [US6] Add `MAX_OUTPUT_BYTES` constant (100KB) and output truncation logic shared across built-in tools in `src/tools/mod.rs`
- [ ] T056 [US6] Ensure all built-in tools check `CancellationToken` before I/O operations for cooperative cancellation in `src/tools/bash.rs`, `src/tools/read_file.rs`, `src/tools/write_file.rs`
- [ ] T057 [US6] Add feature-gated re-exports (`BashTool`, `ReadFileTool`, `WriteFileTool`, `builtin_tools`) in `src/lib.rs` under `#[cfg(feature = "builtin-tools")]`
- [ ] T058 [US6] Add unit tests: BashTool executes command, ReadFileTool reads file, WriteFileTool writes file, builtin_tools() returns all three, cancellation token respected in `src/tools/bash.rs`, `src/tools/read_file.rs`, `src/tools/write_file.rs`
- [ ] T059 [US6] Verify crate compiles with `--no-default-features` (built-in tools excluded) via `cargo test -p swink-agent --no-default-features`

**Checkpoint**: Built-in tools functional and feature-gated — shell and file access available when enabled

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Final integration validation and cross-cutting quality checks

- [ ] T060 Verify full tool dispatch pipeline order (approval → transformer → validator → schema → execute) with integration test in `src/loop_/tool_dispatch.rs`
- [ ] T061 [P] Run `cargo clippy --workspace -- -D warnings` and fix any warnings
- [ ] T062 [P] Run `cargo test --workspace` and verify all tests pass
- [ ] T063 [P] Run `cargo test -p swink-agent --no-default-features` and verify feature-gated compilation
- [ ] T064 Validate quickstart.md examples compile and match public API in `specs/007-tool-system-extensions/quickstart.md`

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **US1 Transformer (Phase 3)**: Depends on Phase 2
- **US2 Validator (Phase 4)**: Depends on Phase 2 (independent of US1)
- **US3 Middleware (Phase 5)**: Depends on Phase 2 (independent of US1/US2)
- **US4 Execution Policy (Phase 6)**: Depends on Phase 2 (independent of US1–US3)
- **US5 FnTool (Phase 7)**: Depends on Phase 2 (independent of US1–US4)
- **US6 Built-in Tools (Phase 8)**: Depends on Phase 1 (feature flag) and Phase 2
- **Polish (Phase 9)**: Depends on all user stories being complete

### User Story Dependencies

- **US1 (P1)**: Foundational only — no cross-story dependencies
- **US2 (P1)**: Foundational only — no cross-story dependencies
- **US3 (P2)**: Foundational only — no cross-story dependencies
- **US4 (P2)**: Foundational only — no cross-story dependencies
- **US5 (P2)**: Foundational only — no cross-story dependencies
- **US6 (P3)**: Foundational + Setup (feature flag) — no cross-story dependencies

### Within Each User Story

- Define types/traits → implement core logic → integrate into loop/config → add tests → re-export from lib.rs

### Parallel Opportunities

- All Setup [P] tasks can run in parallel
- All Foundational [P] tasks can run in parallel (within Phase 2)
- US1 through US5 can proceed in parallel after Phase 2 completes
- US6 can proceed after Phase 1 + Phase 2 complete
- Within US3: with_timeout and with_logging factories are [P]
- Within US6: ReadFileTool and WriteFileTool are [P]

---

## Parallel Example: After Phase 2 Completes

```bash
# All user stories can start simultaneously:
US1: "Define ToolCallTransformer trait in src/tool_call_transformer.rs"
US2: "Define ToolValidator trait in src/tool_validator.rs"
US3: "Define MiddlewareFn type alias in src/tool_middleware.rs"
US4: "Define ToolCallSummary struct in src/tool_execution_policy.rs"
US5: "Define ExecuteFn type alias in src/fn_tool.rs"
US6: "Implement BashTool in src/tools/bash.rs"
```

---

## Implementation Strategy

### MVP First (User Stories 1 + 2 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL — blocks all stories)
3. Complete Phase 3: US1 Transformer
4. Complete Phase 4: US2 Validator
5. **STOP and VALIDATE**: Dispatch pipeline works with transformer + validator

### Incremental Delivery

1. Complete Setup + Foundational → Foundation ready
2. Add US1 Transformer + US2 Validator → Core pipeline (MVP!)
3. Add US3 Middleware → Composable tool decoration
4. Add US4 Execution Policy → Configurable dispatch ordering
5. Add US5 FnTool → Closure-based convenience
6. Add US6 Built-in Tools → Ready-to-use shell/file tools
7. Each story adds value without breaking previous stories

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story is independently testable
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
- Dispatch pipeline order is fixed: approval → transformer → validator → schema → execute
