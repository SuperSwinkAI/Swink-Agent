# Research: Workspace & Cargo Scaffold

**Feature**: 001-workspace-scaffold
**Date**: 2026-03-20

## Technical Context Resolution

No NEEDS CLARIFICATION items in the technical context. All decisions are
pre-determined by the project requirements (MSRV 1.88, edition 2024) and
the existing CLAUDE.md conventions.

## Decisions

### Workspace Layout

- **Decision**: Single root `Cargo.toml` with `[workspace]` defining seven
  `members`. The root crate is also the core library (`swink-agent`).
- **Rationale**: Cargo workspaces with a root-as-member pattern is idiomatic
  for Rust projects where the primary crate lives at the repository root.
  Centralizing `[workspace.dependencies]` in the root `Cargo.toml` ensures
  all subcrates resolve shared libraries to the same version.
- **Alternatives considered**: Virtual workspace (no root package) — rejected
  because the core crate is the primary deliverable and benefits from being
  at the root for discoverability.

### Dependency Centralization

- **Decision**: Use `[workspace.dependencies]` in root `Cargo.toml` with
  `.workspace = true` in subcrate dependencies.
- **Rationale**: Cargo's built-in workspace dependency inheritance (stable
  since Rust 1.64) is the standard approach. Avoids version drift between
  crates.
- **Alternatives considered**: Per-crate version pinning — rejected because
  it creates maintenance burden and risks version conflicts.

### Toolchain Pinning

- **Decision**: `rust-toolchain.toml` with `channel = "1.88"`.
- **Rationale**: `rust-toolchain.toml` is the standard rustup mechanism for
  automatic toolchain selection. TOML format is preferred over the legacy
  `rust-toolchain` plain text file.
- **Alternatives considered**: Relying solely on `rust-version` in
  `Cargo.toml` — rejected because it only produces errors, it does not
  auto-install the correct toolchain.

### Linting Configuration

- **Decision**: `[lints.rust]` and `[lints.clippy]` sections in root
  `Cargo.toml` with `unsafe_code = "forbid"`, `clippy::all`, `pedantic`,
  and `nursery` as warnings, plus targeted allows for noisy lints
  (`module_name_repetitions`, `must_use_candidate`, `missing_errors_doc`,
  `missing_panics_doc`).
- **Rationale**: Workspace-level lint configuration via `Cargo.toml` (stable
  since Rust 1.74) keeps linting rules in one place. The three clippy groups
  provide comprehensive coverage while the targeted allows suppress lints
  that conflict with the project's API style.
- **Alternatives considered**: `.clippy.toml` file — rejected because
  `Cargo.toml` lints are more discoverable and workspace-inheritable.

### Feature Gates

- **Decision**: `builtin-tools` feature flag on the core crate, enabled by
  default. Gates `BashTool`, `ReadFileTool`, `WriteFileTool`.
- **Rationale**: Downstream consumers who don't need built-in tools can
  disable them to reduce compile time and binary size. Default-on ensures
  the common case works out of the box.
- **Alternatives considered**: Separate crate for built-in tools — rejected
  because the tools are tightly coupled to core types and the feature flag
  achieves the same goal with less overhead.

### Xtask Convention

- **Decision**: Empty `main()` in the xtask crate for this scaffold.
  Subcommands deferred to later features.
- **Rationale**: Clarification session resolved this — the scaffold should
  be purely structural. The xtask crate exists as a workspace member to
  establish the pattern; commands are added as needed.
- **Alternatives considered**: CLI skeleton with clap — rejected per
  clarification (option A selected).
