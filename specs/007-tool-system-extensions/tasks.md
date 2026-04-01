# Tasks: Tool System Extensions

**Input**: Design documents from `/specs/007-tool-system-extensions/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/public-api.md

> **Supersession Note**: Tasks for US1 (ToolCallTransformer) and US2 (ToolValidator) were completed as the original design. These types are superseded by `PreDispatchPolicy` (Slot 2) in [031-policy-slots](../031-policy-slots/spec.md). The dispatch pipeline order changes from "approval → transformer → validator → schema → execute" to "PreDispatch policies → approval → schema validation → execute." The code delivered by these tasks remains pending 031 implementation. Tasks for US3-US6 (ToolMiddleware, ExecutionPolicy, FnTool, Built-in Tools) remain active and unaffected.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

**TDD Note**: Per constitution principle II (Test-Driven Development), test tasks within each phase MUST be executed before their corresponding implementation tasks, regardless of task ID ordering. Write tests first, verify they fail, then implement.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Feature flag configuration and shared type preparation

- [x] T001 Add `builtin-tools` feature flag (default-enabled) to `Cargo.toml` with `tokio/process` dep gated behind it
- [x] T002 [P] Add `regex` workspace dependency to root `Cargo.toml` for sensitive value pattern matching
- [x] T003 [P] Create `src/tools/` directory with `src/tools/mod.rs` for feature-gated built-in tool re-exports

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core types and validation functions in `src/tool.rs` that ALL user stories depend on

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [x] T004 Implement `ToolMetadata` struct with `namespace`/`version` fields and `with_namespace()`/`with_version()` builders in `src/tool.rs`
- [x] T005 Implement `ToolApproval` enum (`Approved`, `Rejected`, `ApprovedWith(Value)`) in `src/tool.rs`
- [x] T006 Implement `ToolApprovalRequest` struct with redacted `Debug` impl in `src/tool.rs`
- [x] T007 Implement `ApprovalMode` enum (`Enabled`, `Smart`, `Bypassed`) with `Default` derive in `src/tool.rs`
- [x] T008 Implement `validate_schema()` and `validate_tool_arguments()` functions using `jsonschema` crate in `src/tool.rs`
- [x] T009 [P] Implement `unknown_tool_result()` and `validation_error_result()` helper constructors in `src/tool.rs`
- [x] T010 [P] Implement `redact_sensitive_values()` with regex-based pattern matching for keys (password, secret, token, api_key) and value prefixes (sk-, key-, bearer) in `src/tool.rs`
- [x] T011 Implement `selective_approve()` helper that wraps an approval callback with `ApprovalMode` filtering in `src/tool.rs`
- [x] T012 Add `AgentTool` trait methods `requires_approval()` (default false) and `metadata()` (default None) to `src/tool.rs`

**Checkpoint**: Foundation ready — tool system types available for all user stories

---

## Phase 3: User Story 1 — Rewrite Tool Calls Before Execution (Priority: P1) 🎯 MVP

**Goal**: Enable developers to register a transformer that rewrites tool call arguments before validation and execution

**Independent Test**: Register a transformer that modifies an argument, invoke a tool, verify the tool receives modified arguments

### Implementation for User Story 1

- [x] T013 [US1] Define `ToolCallTransformer` trait with `transform(&self, tool_name: &str, arguments: &mut Value)` method in `src/tool_call_transformer.rs`
- [x] T014 [US1] Implement blanket impl of `ToolCallTransformer` for `Fn(&str, &mut Value) + Send + Sync` closures in `src/tool_call_transformer.rs`
- [x] T015 [US1] Add `tool_call_transformer: Option<Arc<dyn ToolCallTransformer>>` field to `AgentLoopConfig` in `src/loop_/mod.rs`
- [x] T016 [US1] Integrate transformer into tool dispatch pipeline (after approval, before validator) in `src/loop_/tool_dispatch.rs`
- [x] T017 [US1] Add unit tests: transformer modifies args, no transformer passes through, closure blanket impl works in `src/tool_call_transformer.rs`
- [x] T018 [US1] Re-export `ToolCallTransformer` from `src/lib.rs`

**Checkpoint**: Transformer pipeline functional — arguments can be rewritten before validation

---

## Phase 4: User Story 2 — Validate Tool Calls Before Execution (Priority: P1)

**Goal**: Enable developers to register a validator that accepts or rejects tool calls after transformation

**Independent Test**: Register a validator that rejects a specific tool name, verify the tool is not executed

### Implementation for User Story 2

- [x] T019 [US2] Define `ToolValidator` trait with `validate(&self, tool_name: &str, arguments: &Value) -> Result<(), String>` method in `src/tool_validator.rs`
- [x] T020 [US2] Implement blanket impl of `ToolValidator` for `Fn(&str, &Value) -> Result<(), String> + Send + Sync` closures in `src/tool_validator.rs`
- [x] T021 [US2] Add `tool_validator: Option<Arc<dyn ToolValidator>>` field to `AgentLoopConfig` in `src/loop_/mod.rs`
- [x] T022 [US2] Integrate validator into tool dispatch pipeline (after transformer, before schema validation) in `src/loop_/tool_dispatch.rs`
- [x] T023 [US2] Add unit tests: validator accepts, validator rejects with error result, closure blanket impl works in `src/tool_validator.rs`
- [x] T024 [US2] Re-export `ToolValidator` from `src/lib.rs`

**Checkpoint**: Validator pipeline functional — tool calls can be rejected before execution

---

## Phase 5: User Story 3 — Wrap Tool Execution with Middleware (Priority: P2)

**Goal**: Enable developers to wrap a tool's execute function with composable middleware for cross-cutting concerns

**Independent Test**: Wrap a tool with logging middleware, verify middleware runs around the tool's execute call

### Implementation for User Story 3

- [x] T025 [US3] Define `MiddlewareFn` type alias for the middleware closure signature in `src/tool_middleware.rs`
- [x] T026 [US3] Implement `ToolMiddleware` struct with `inner: Arc<dyn AgentTool>` and `middleware_fn: Arc<MiddlewareFn>` fields in `src/tool_middleware.rs`
- [x] T027 [US3] Implement `ToolMiddleware::new()` constructor accepting inner tool and closure in `src/tool_middleware.rs`
- [x] T028 [US3] Implement `AgentTool` for `ToolMiddleware` — delegate metadata methods to inner, intercept `execute()` in `src/tool_middleware.rs`
- [x] T029 [P] [US3] Implement `ToolMiddleware::with_timeout()` factory method using `tokio::time::timeout` in `src/tool_middleware.rs`
- [x] T030 [P] [US3] Implement `ToolMiddleware::with_logging()` factory method calling callback with (name, id, is_start) in `src/tool_middleware.rs`
- [x] T031 [US3] Add unit tests: middleware intercepts execute, metadata delegates to inner, with_timeout enforces limit, with_logging calls callback in `src/tool_middleware.rs`
- [x] T032 [US3] Re-export `ToolMiddleware` from `src/lib.rs`

**Checkpoint**: Middleware functional — tools can be decorated with composable wrappers

---

## Phase 6: User Story 4 — Control Tool Execution Order (Priority: P2)

**Goal**: Enable developers to configure execution policy for batched tool calls (concurrent, sequential, or priority-based)

**Independent Test**: Configure sequential policy, verify tools execute one after another rather than concurrently

### Implementation for User Story 4

- [x] T033 [US4] Define `ToolCallSummary<'a>` borrowed view struct with `id`, `name`, `arguments` fields in `src/tool_execution_policy.rs`
- [x] T034 [US4] Define `PriorityFn` type alias (`dyn Fn(&ToolCallSummary) -> i32 + Send + Sync`) in `src/tool_execution_policy.rs`
- [x] T035 [US4] Define `ToolExecutionStrategy` trait with async `partition()` method returning `Vec<Vec<usize>>` in `src/tool_execution_policy.rs`
- [x] T036 [US4] Implement `ToolExecutionPolicy` enum with `Concurrent`, `Sequential`, `Priority`, `Custom` variants in `src/tool_execution_policy.rs`
- [x] T037 [US4] Implement `Clone` and `Debug` manually for `ToolExecutionPolicy` (Arc fields require manual impls) in `src/tool_execution_policy.rs`
- [x] T038 [US4] Add `tool_execution_policy: ToolExecutionPolicy` field to `AgentLoopConfig` in `src/loop_/mod.rs`
- [x] T039 [US4] Integrate execution policy into tool dispatch — compute groups from policy, execute groups sequentially with concurrent within-group in `src/loop_/tool_dispatch.rs`
- [x] T040 [US4] Add unit tests: concurrent runs all at once, sequential runs in order, priority groups by value, custom strategy partitions in `src/tool_execution_policy.rs`
- [x] T041 [US4] Re-export `ToolCallSummary`, `PriorityFn`, `ToolExecutionPolicy`, `ToolExecutionStrategy` from `src/lib.rs`

**Checkpoint**: Execution policies functional — tool dispatch order is configurable

---

## Phase 7: User Story 5 — Create Tools from Closures (Priority: P2)

**Goal**: Enable developers to create tools from closures without defining a full struct

**Independent Test**: Create a closure-based tool, register it, verify it executes correctly when called

### Implementation for User Story 5

- [x] T042 [US5] Define `ExecuteFn` type alias for the stored execution closure in `src/fn_tool.rs`
- [x] T043 [US5] Implement `FnTool` struct with `name`, `label`, `description`, `schema`, `requires_approval`, `execute_fn` fields in `src/fn_tool.rs`
- [x] T044 [US5] Implement `FnTool::new()` constructor with default schema (accepts any object) and stub execute in `src/fn_tool.rs`
- [x] T045 [US5] Implement builder methods `with_schema_for::<T: JsonSchema>()`, `with_schema(Value)`, `with_requires_approval(bool)` in `src/fn_tool.rs`
- [x] T046 [US5] Implement `with_execute()` (full signature) and `with_execute_simple()` (params + cancel only) builder methods in `src/fn_tool.rs`
- [x] T047 [US5] Implement `AgentTool` for `FnTool` — delegate to stored fields and closure in `src/fn_tool.rs`
- [x] T048 [US5] Implement `Debug` for `FnTool` (closure fields use opaque display) in `src/fn_tool.rs`
- [x] T049 [US5] Add unit tests: FnTool executes closure, with_schema_for derives schema, with_execute_simple works, trait methods delegate correctly in `src/fn_tool.rs`
- [x] T050 [US5] Re-export `FnTool` from `src/lib.rs`

**Checkpoint**: FnTool functional — tools can be created from closures with zero boilerplate

---

## Phase 8: User Story 6 — Use Built-In Shell and File Tools (Priority: P3)

**Goal**: Provide pre-made tools for shell execution, file reading, and file writing behind a feature gate

**Independent Test**: Enable the feature flag, register built-in tools, verify they execute correctly

### Implementation for User Story 6

- [x] T051 [US6] Implement `BashTool` struct with pre-computed JSON Schema, `new()`/`Default`, and `AgentTool` impl (name: "bash", requires_approval: true) using `tokio::process::Command` in `src/tools/bash.rs`
- [x] T052 [P] [US6] Implement `ReadFileTool` struct with JSON Schema, `new()`/`Default`, and `AgentTool` impl (name: "read_file", requires_approval: false) using `tokio::fs::read_to_string` in `src/tools/read_file.rs`
- [x] T053 [P] [US6] Implement `WriteFileTool` struct with JSON Schema, `new()`/`Default`, and `AgentTool` impl (name: "write_file", requires_approval: true) using `tokio::fs::write` in `src/tools/write_file.rs`
- [x] T054 [US6] Implement `builtin_tools()` convenience function returning `Vec<Arc<dyn AgentTool>>` with all three tools in `src/tools/mod.rs`
- [x] T055 [US6] Add `MAX_OUTPUT_BYTES` constant (100KB) and output truncation logic shared across built-in tools in `src/tools/mod.rs`
- [x] T056 [US6] Ensure all built-in tools check `CancellationToken` before I/O operations for cooperative cancellation in `src/tools/bash.rs`, `src/tools/read_file.rs`, `src/tools/write_file.rs`
- [x] T057 [US6] Add feature-gated re-exports (`BashTool`, `ReadFileTool`, `WriteFileTool`, `builtin_tools`) in `src/lib.rs` under `#[cfg(feature = "builtin-tools")]`
- [x] T058 [US6] Add unit tests: BashTool executes command, ReadFileTool reads file, WriteFileTool writes file, builtin_tools() returns all three, cancellation token respected in `src/tools/bash.rs`, `src/tools/read_file.rs`, `src/tools/write_file.rs`
- [x] T059 [US6] Verify crate compiles with `--no-default-features` (built-in tools excluded) via `cargo test -p swink-agent --no-default-features`

**Checkpoint**: Built-in tools functional and feature-gated — shell and file access available when enabled

---

## Phase 9: User Story 7 — Auto-Schema Generation (Priority: P1) — C12

**Goal**: Proc macros for generating JSON Schema from Rust types and wrapping async functions as `AgentTool` implementations

**Independent Test**: Define a struct with `#[derive(ToolSchema)]`, call `json_schema()`, verify correct JSON Schema output

### Implementation for User Story 7

- [x] T065 [US7] Create `macros/` directory with `Cargo.toml` declaring `proc-macro = true`, deps: `syn`, `quote`, `proc-macro2`. Add to workspace members.
- [x] T066 [US7] Define `ToolParameters` trait with `fn json_schema() -> Value` in `src/tool.rs` (or `src/tool_parameters.rs`). Re-export from `src/lib.rs`.
- [x] T067 [US7] Implement `#[derive(ToolSchema)]` proc macro in `macros/src/tool_schema.rs`: map field types to JSON Schema types, extract doc comments as descriptions, support `#[tool(description = "...")]` override.
- [x] T068 [US7] Implement `#[tool]` attribute macro in `macros/src/tool_attr.rs`: generate struct + `AgentTool` impl from async function signature with `name` and `description` attributes.
- [x] T069 [US7] Wire up `macros/src/lib.rs` to export both proc macros.
- [x] T070 [US7] Add unit tests in `macros/tests/`: `derive_tool_schema_basic` (String/u64/bool fields), `derive_tool_schema_option` (Option field not required), `derive_tool_schema_vec` (Vec → array), `derive_tool_schema_doc_comments` (doc comments → description), `derive_tool_schema_attr_override` (`#[tool(description = "...")]` overrides doc comment).
- [x] T071 [US7] Add unit tests in `macros/tests/`: `tool_attr_generates_struct` (function becomes AgentTool), `tool_attr_schema_from_params` (schema matches function params), `tool_attr_requires_async` (non-async fn = compile error), `derive_tool_schema_unsupported_type` (HashMap or custom enum produces compile error with helpful message).
- [x] T072 [US7] Add integration test: create an `AgentTool` via `#[tool]` macro, register it with an agent, verify it executes correctly.

**Checkpoint**: Proc macros functional — tools can be created from structs and functions with zero schema boilerplate

---

## Phase 10: User Story 8 — Tool Hot-Reloading (Priority: P2) — I12

**Goal**: Feature-gated directory watcher that loads tool definitions from files and updates the agent at runtime

**Independent Test**: Start a watcher on a temp directory, add a tool definition file, verify the agent's tool list updates

### Implementation for User Story 8

- [x] T073 [US8] Add `hot-reload` feature gate to `Cargo.toml` with optional `notify` dependency.
- [x] T074 [US8] Implement `ScriptTool` struct in `src/hot_reload.rs`: parse TOML/YAML/JSON definition files, implement `AgentTool` with shell command execution.
- [x] T075 [US8] Implement `ToolWatcher` struct in `src/hot_reload.rs`: use `notify` to watch directory, debounce changes, load/reload/remove `ScriptTool` instances.
- [x] T076 [US8] Implement `ToolWatcher::start()` — spawns async task, watches for file events, calls `Agent::set_tools()` on changes. Apply optional `ToolFilter`.
- [x] T077 [US8] Add unit tests: `script_tool_from_toml` (parse TOML definition), `script_tool_executes_command` (run command with interpolated args), `script_tool_invalid_definition` (reject malformed files), `script_tool_escapes_parameters` (verify parameter values are shell-escaped before interpolation — e.g., `"; rm -rf /"` in a parameter does not execute as a command).
- [x] T078 [US8] Add integration test: start watcher on tempdir, add/modify/delete TOML files, verify tool list updates. Include `duplicate_tool_names_last_write_wins` — add two files defining the same tool name, verify the most recently modified one takes precedence with a warning logged.
- [x] T079 [US8] Re-export `ToolWatcher` and `ScriptTool` from `src/lib.rs` behind `#[cfg(feature = "hot-reload")]`.

**Checkpoint**: Hot-reloading functional — tools can be added/modified/removed via definition files at runtime

---

## Phase 11: User Story 9 — Tool Filtering (Priority: P2) — I13

**Goal**: Pattern-based tool filtering at registration time using exact, glob, and regex patterns

**Independent Test**: Create a `ToolFilter`, register tools, verify only matching tools are available

### Implementation for User Story 9

- [x] T080 [US9] Implement `ToolPattern` enum (Exact/Glob/Regex) with `parse()` auto-detection and `matches()` in `src/tool_filter.rs`.
- [x] T081 [US9] Implement `ToolFilter` struct with `allowed`/`rejected` fields, `with_allowed()`/`with_rejected()` builders, and `filter_tools()` method in `src/tool_filter.rs`.
- [x] T082 [US9] Add unit tests: `exact_pattern_matches` , `glob_pattern_matches` (`read_*` matches `read_file`), `regex_pattern_matches` (`^file_.*$`), `rejected_takes_precedence`, `empty_filter_allows_all`.
- [x] T083 [US9] Re-export `ToolFilter` and `ToolPattern` from `src/lib.rs`.

**Checkpoint**: Tool filtering functional — registration-time pattern matching restricts available tools

---

## Phase 12: User Story 10 — Noop Tool (Priority: P3) — N5

**Goal**: Auto-inject placeholder tools for session history compatibility

**Independent Test**: Load a session referencing a non-existent tool, verify NoopTool is injected and returns error result

### Implementation for User Story 10

- [x] T084 [US10] Implement `NoopTool` struct in `src/noop_tool.rs`: `new(name)`, `AgentTool` impl returning error result.
- [x] T085 [US10] Integrate `NoopTool` injection into session loading — detect tool references not in registry, auto-inject NoopTool for each.
- [x] T086 [US10] Add unit tests: `noop_tool_returns_error`, `noop_tool_name_matches`, `noop_tool_no_approval_required`.
- [x] T087 [US10] Re-export `NoopTool` from `src/lib.rs`.

**Checkpoint**: NoopTool functional — sessions with removed tools load gracefully

---

## Phase 13: User Story 11 — Tool Confirmation Payloads (Priority: P3) — N6

**Goal**: Rich context for the approval UI via `approval_context()` default method on AgentTool

**Independent Test**: Implement `approval_context` on a tool, trigger approval, verify context attached to request

### Implementation for User Story 11

- [x] T088 [US11] Add `fn approval_context(&self, params: &Value) -> Option<Value> { None }` default method to `AgentTool` trait in `src/tool.rs`.
- [x] T089 [US11] Add `context: Option<Value>` field to `ToolApprovalRequest` in `src/tool.rs`. Populate from `approval_context()` in tool dispatch pipeline (`src/loop_/tool_dispatch.rs`).
- [x] T090 [US11] Add `catch_unwind` around `approval_context()` call — log panics, set context to `None`.
- [x] T091 [US11] Update `FnTool` to support `approval_context` via a new `with_approval_context()` builder method.
- [x] T092 [US11] Add unit tests: `approval_context_default_none`, `approval_context_returns_value`, `approval_context_panic_caught`, `approval_request_includes_context`.
- [x] T093 [US11] Update `ToolMiddleware` to delegate `approval_context()` to inner tool (same as other metadata methods).

**Checkpoint**: Approval context functional — tools can provide rich previews to the approval UI

---

## Phase 14: Polish & Cross-Cutting Concerns

**Purpose**: Final integration validation and cross-cutting quality checks

- [x] T060 Verify full tool dispatch pipeline order (approval → transformer → validator → schema → execute) with integration test in `src/loop_/tool_dispatch.rs`
- [x] T061 [P] Run `cargo clippy --workspace -- -D warnings` and fix any warnings
- [x] T062 [P] Run `cargo test --workspace` and verify all tests pass
- [x] T063 [P] Run `cargo test -p swink-agent --no-default-features` and verify feature-gated compilation
- [x] T064 Validate quickstart.md examples compile and match public API in `specs/007-tool-system-extensions/quickstart.md`
- [x] T094 Add compile-time `Send + Sync` assertions for new public types: `ToolFilter`, `ToolPattern`, `NoopTool`, `ScriptTool` (behind feature gate)
- [x] T095 Run `cargo build -p swink-agent --features hot-reload` and verify zero compilation errors
- [x] T096 Run `cargo test -p swink-agent-macros` and verify all macro tests pass
- [x] T097 Run `cargo clippy --workspace -- -D warnings` with all features enabled and fix any warnings
- [x] T098 Validate quickstart.md new examples (auto-schema, filtering, hot-reload, noop, confirmation) match actual API

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
- **US7 Auto-Schema (Phase 9)**: Depends on Phase 2 (needs `ToolParameters` trait in core). Independent of US1–US6.
- **US8 Hot-Reloading (Phase 10)**: Depends on Phase 2. Optionally uses US9 (ToolFilter) but can proceed independently.
- **US9 Tool Filtering (Phase 11)**: Depends on Phase 2. Independent of all other stories.
- **US10 Noop Tool (Phase 12)**: Depends on Phase 2. Independent of all other stories.
- **US11 Confirmation Payloads (Phase 13)**: Depends on Phase 2. Touches `src/tool.rs` (AgentTool trait) and `src/loop_/tool_dispatch.rs`.
- **Polish (Phase 14)**: Depends on all user stories being complete

### User Story Dependencies

- **US1 (P1)**: Foundational only — no cross-story dependencies
- **US2 (P1)**: Foundational only — no cross-story dependencies
- **US3 (P2)**: Foundational only — no cross-story dependencies
- **US4 (P2)**: Foundational only — no cross-story dependencies
- **US5 (P2)**: Foundational only — no cross-story dependencies
- **US6 (P3)**: Foundational + Setup (feature flag) — no cross-story dependencies
- **US7 (P1)**: Foundational + new macros crate — no cross-story dependencies
- **US8 (P2)**: Foundational + feature flag — optionally uses US9 ToolFilter
- **US9 (P2)**: Foundational only — no cross-story dependencies
- **US10 (P3)**: Foundational only — no cross-story dependencies
- **US11 (P3)**: Foundational only — modifies AgentTool trait (default method, backward compatible)

### Within Each User Story

- Define types/traits → implement core logic → integrate into loop/config → add tests → re-export from lib.rs

### Parallel Opportunities

- All Setup [P] tasks can run in parallel
- All Foundational [P] tasks can run in parallel (within Phase 2)
- US1 through US5 can proceed in parallel after Phase 2 completes
- US6 can proceed after Phase 1 + Phase 2 complete
- US7 through US11 can proceed in parallel after Phase 2 (US7 needs macros crate setup first)
- Within US3: with_timeout and with_logging factories are [P]
- Within US6: ReadFileTool and WriteFileTool are [P]
- US9 (ToolFilter) and US10 (NoopTool) and US11 (Confirmation Payloads) touch separate files with no cross-deps

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
US7: "Create macros/ crate, define ToolParameters trait"
US8: "Add hot-reload feature gate, implement ScriptTool"
US9: "Implement ToolFilter in src/tool_filter.rs"
US10: "Implement NoopTool in src/noop_tool.rs"
US11: "Add approval_context() to AgentTool trait"
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
7. Add US7 Auto-Schema → Zero-boilerplate tool definition
8. Add US11 Confirmation Payloads → Rich approval context
9. Add US9 Tool Filtering → Registration-time security
10. Add US10 Noop Tool → Session compatibility
11. Add US8 Hot-Reloading → Runtime tool management
12. Each story adds value without breaking previous stories

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story is independently testable
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
- Dispatch pipeline order is fixed: approval → transformer → validator → schema → execute
- US7 (Auto-Schema) is new work — creates the `swink-agent-macros` workspace crate
- US8 (Hot-Reloading) is new work — feature-gated behind `hot-reload`
- US9 (Tool Filtering), US10 (Noop Tool), US11 (Confirmation Payloads) are new work in core crate
