# Data Model: Workspace & Cargo Scaffold

**Feature**: 001-workspace-scaffold
**Date**: 2026-03-20

## Overview

The scaffold feature defines structural entities only — no runtime data
types, state machines, or persistence. The "data model" for this feature
is the workspace topology: crate identities, their relationships, and
their configuration.

## Entities

### Workspace

The root container that defines all crate members and shared settings.

| Attribute | Value | Location |
|-----------|-------|----------|
| resolver | "2" | `Cargo.toml` `[workspace]` |
| members | 7 crates (see below) | `Cargo.toml` `[workspace].members` |
| shared dependencies | Centralized versions | `Cargo.toml` `[workspace.dependencies]` |

### Crates

| Crate Name | Package Name | Type | Entry Point | Depends On |
|---|---|---|---|---|
| `.` (root) | `swink-agent` | lib | `src/lib.rs` | (none — this is core) |
| `adapters/` | `swink-agent-adapters` | lib | `adapters/src/lib.rs` | `swink-agent` |
| `memory/` | `swink-agent-memory` | lib | `memory/src/lib.rs` | `swink-agent` |
| `local-llm/` | `swink-agent-local-llm` | lib | `local-llm/src/lib.rs` | `swink-agent` |
| `eval/` | `swink-agent-eval` | lib | `eval/src/lib.rs` | `swink-agent` |
| `tui/` | `swink-agent-tui` | bin+lib | `tui/src/main.rs`, `tui/src/lib.rs` | `swink-agent`, `swink-agent-adapters`, `swink-agent-memory`, `swink-agent-local-llm` |
| `xtask/` | `xtask` | bin | `xtask/src/main.rs` | (none) |

### Dependency Graph

```text
swink-agent (core)
├── swink-agent-adapters
├── swink-agent-memory
├── swink-agent-local-llm
├── swink-agent-eval
└── swink-agent-tui
    ├── swink-agent-adapters
    ├── swink-agent-memory
    └── swink-agent-local-llm

xtask (independent)
```

All arrows point downward (toward core). No reverse or circular
dependencies.

### Configuration Files

| File | Purpose |
|---|---|
| `Cargo.toml` | Workspace definition, core crate package, shared deps, lint config, build profiles |
| `rust-toolchain.toml` | Pins Rust 1.88 for automatic toolchain selection |
| `rustfmt.toml` | Formatter rules for consistent code style |
| `.gitignore` | Excludes `target/`, `.env`, editor files, OS artifacts |

### Feature Flags

| Crate | Flag | Default | Gates |
|---|---|---|---|
| `swink-agent` | `builtin-tools` | enabled | `BashTool`, `ReadFileTool`, `WriteFileTool` |
| `swink-agent` | `test-helpers` | disabled | Shared test utilities for downstream crates |

## Relationships

- Every library crate's `lib.rs` re-exports the public API surface
- The TUI crate has both `lib.rs` (for testable components) and `main.rs` (entry point)
- The xtask crate has only `main.rs` (empty in this scaffold)
- All library crates include `#[forbid(unsafe_code)]` at the crate root

## Validation Rules

- FR-010 defines the allowed dependency edges — any dependency not in the
  graph above is a violation
- SC-005 requires verification that no crate depends on a crate it should not
- SC-006 requires that `swink-agent` alone does not transitively pull in
  adapters, memory, eval, local-llm, or TUI dependencies
