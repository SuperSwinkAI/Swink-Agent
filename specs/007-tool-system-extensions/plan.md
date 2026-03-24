# Implementation Plan: Tool System Extensions

**Branch**: `007-tool-system-extensions` | **Date**: 2026-03-20 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/007-tool-system-extensions/spec.md`

## Supersession Notice

> **Partially superseded by [031-policy-slots](../031-policy-slots/spec.md).**
> `ToolCallTransformer` and `ToolValidator` are replaced by `PreDispatchPolicy` (Slot 2) in the 031 policy slot system. The dispatch pipeline order changes from "approval → transformer → validator → schema → execute" to "PreDispatch policies → approval → schema validation → execute."
> `ToolMiddleware`, `ToolExecutionPolicy`, `FnTool`, and built-in tools remain valid and active.

## Summary

Extend the core tool system with composable pipeline hooks and convenience abstractions. The feature adds components to the `swink-agent` core crate: ~~a `ToolCallTransformer` trait for pre-validation argument rewriting, a `ToolValidator` trait for accept/reject gating,~~ **[Superseded by 031 PreDispatchPolicy]** a `ToolMiddleware` decorator for wrapping `execute()`, a `ToolExecutionPolicy` enum controlling dispatch concurrency, a `FnTool` closure-based tool builder, and three built-in tools (`BashTool`, `ReadFileTool`, `WriteFileTool`) behind the `builtin-tools` feature gate. ~~The dispatch pipeline is fixed: approval, transformer, validator, schema validation, execute.~~ **[Superseded by 031]** New order: PreDispatch policies → approval → schema validation → execute.

## Technical Context

**Language/Version**: Rust 1.88, edition 2024
**Primary Dependencies**: `tokio`, `tokio-util` (CancellationToken), `serde_json`, `schemars`, `jsonschema`, `regex`
**Storage**: N/A
**Testing**: `cargo test --workspace`, `cargo test -p swink-agent --no-default-features`
**Target Platform**: Cross-platform library crate
**Project Type**: Library
**Performance Goals**: Concurrent tool dispatch via `tokio::spawn`; zero-copy borrowed views in `ToolCallSummary`
**Constraints**: `#[forbid(unsafe_code)]`, no provider-specific or UI-specific dependencies in core
**Scale/Scope**: 8 source files across `src/` and `src/tools/`

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|---|---|---|
| I. Library-First | PASS | All components live in the `swink-agent` core crate as public API surface. No new crates introduced. Built-in tools are feature-gated to keep core dependency-free when unused. |
| II. Test-Driven Development | PASS | Each source file includes inline `#[cfg(test)]` modules. Tests cover closure blanket impls, middleware delegation, policy variants, and built-in tool execution. `cargo test --workspace` and `--no-default-features` both required to pass. |
| III. Efficiency & Performance | PASS | Default execution policy is concurrent via `tokio::spawn`. `ToolCallSummary` uses borrowed references to avoid cloning arguments. `Arc` used for middleware and policy closures to enable zero-copy sharing. |
| IV. Leverage the Ecosystem | PASS | Uses `schemars` for schema derivation, `jsonschema` for validation, `tokio::process` for shell execution. No hand-rolled alternatives. |
| V. Provider Agnosticism | PASS | All components are provider-agnostic. No API keys, SDK clients, or provider types. The tool system is orthogonal to `StreamFn`. |
| VI. Safety & Correctness | PASS | `#[forbid(unsafe_code)]` enforced. Panics in spawned tool tasks caught via join error handling. Cancellation tokens for cooperative abort. `AgentToolResult::error()` for error signaling, never panics. |

## Project Structure

### Documentation (this feature)

```text
specs/007-tool-system-extensions/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── public-api.md
└── tasks.md             # Phase 2 output (NOT created by plan)
```

### Source Code (repository root)

```text
src/
├── tool.rs                      # AgentTool trait, AgentToolResult, validation, approval
├── tool_call_transformer.rs     # ToolCallTransformer trait + blanket closure impl
├── tool_validator.rs            # ToolValidator trait + blanket closure impl
├── tool_middleware.rs           # ToolMiddleware decorator (wraps execute)
├── tool_execution_policy.rs     # ToolExecutionPolicy enum, ToolExecutionStrategy trait
├── fn_tool.rs                   # FnTool closure-based tool builder
├── tools/
│   ├── mod.rs                   # Feature-gated re-exports, builtin_tools()
│   ├── bash.rs                  # BashTool (sh -c execution)
│   ├── read_file.rs             # ReadFileTool (file reading)
│   └── write_file.rs            # WriteFileTool (file writing)
├── lib.rs                       # Public re-exports for all tool system types
└── loop_.rs                     # Agent loop — consumes the dispatch pipeline

Cargo.toml                       # Feature flag: builtin-tools (default)
```

**Structure Decision**: All tool system extensions are in the core crate (`swink-agent`). Each concern has its own file (one concern per file). Built-in tools are in the `src/tools/` subdirectory behind a feature gate. Re-exports in `lib.rs` ensure consumers never reach into submodules.

## Complexity Tracking

No constitution violations. All components fit within existing crate boundaries.
