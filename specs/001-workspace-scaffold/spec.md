# Feature Specification: Workspace & Cargo Scaffold

**Feature Branch**: `001-workspace-scaffold`
**Created**: 2026-03-20
**Status**: Draft
**Input**: Foundational workspace structure for a 7-crate Rust workspace providing the scaffolding for an LLM-powered agent library.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Library Consumer Adds Dependency (Priority: P1)

A developer building an LLM-powered application adds the core `swink-agent` crate as a dependency. They import the public API from the crate root and their project compiles without errors. The crate exposes a clean, minimal public surface with no transitive dependency on provider-specific or UI-specific crates.

**Why this priority**: This is the fundamental value proposition — the workspace must produce a usable library crate that downstream consumers can depend on. Without this, nothing else works.

**Independent Test**: Can be tested by creating a minimal Rust project that depends on `swink-agent` and verifying it compiles with a single `use swink_agent::*;` import.

**Acceptance Scenarios**:

1. **Given** a new Rust project, **When** the developer adds `swink-agent` as a dependency, **Then** the project compiles and the public API types are accessible via the crate root.
2. **Given** the `swink-agent` crate, **When** compiled with default features, **Then** builtin tools (Bash, ReadFile, WriteFile) are included.
3. **Given** the `swink-agent` crate, **When** compiled with `default-features = false`, **Then** builtin tools are excluded and the crate still compiles cleanly.

---

### User Story 2 - Workspace Developer Builds All Crates (Priority: P1)

A contributor clones the repository and runs a single workspace-wide build command. All seven crates compile successfully with zero warnings under the project's strict linting policy. Dependency versions are centralized so there are no version conflicts between crates.

**Why this priority**: Contributors must be able to build the entire workspace from a clean checkout. This validates the structural integrity of the workspace layout and inter-crate dependency graph.

**Independent Test**: Can be tested by running the workspace build command and the workspace lint command on a clean checkout and verifying zero errors and zero warnings.

**Acceptance Scenarios**:

1. **Given** a clean repository checkout, **When** the developer runs the workspace build command, **Then** all seven crates compile without errors.
2. **Given** the workspace, **When** the linter runs with warnings-as-errors, **Then** zero warnings are reported.
3. **Given** the workspace, **When** two crates depend on the same external library, **Then** both resolve to the same version via centralized dependency management.

---

### User Story 3 - Adapter Author Adds a New Provider (Priority: P2)

A developer creating a new LLM provider adapter adds a module to the adapters crate. They can depend on core types from `swink-agent` without pulling in memory, eval, local-llm, or TUI dependencies. The adapter crate's dependency on the core is clearly expressed and the new module slots into the existing crate structure.

**Why this priority**: The workspace must enforce clean dependency boundaries between crates so that adding provider-specific code does not bloat unrelated crates.

**Independent Test**: Can be tested by verifying that `swink-agent-adapters` depends only on `swink-agent` core (not on memory, eval, local-llm, or TUI) via dependency inspection.

**Acceptance Scenarios**:

1. **Given** the adapters crate, **When** its dependency tree is inspected, **Then** it depends on `swink-agent` core but not on `swink-agent-memory`, `swink-agent-local-llm`, `swink-agent-eval`, or `swink-agent-tui`.
2. **Given** the workspace, **When** the TUI crate is examined, **Then** it depends on core, adapters, memory, and local-llm as expected by the dependency chain.

---

### User Story 4 - Toolchain Consistency Across Environments (Priority: P2)

A contributor working on a different machine checks out the repository and gets the correct Rust toolchain version automatically. The minimum supported Rust version, edition, and formatting rules are pinned in configuration files so that all contributors produce consistent output regardless of their local toolchain defaults.

**Why this priority**: Toolchain consistency prevents "works on my machine" issues and ensures CI and local builds behave identically.

**Independent Test**: Can be tested by verifying that the toolchain configuration file pins the expected Rust version and that the formatter and linter produce deterministic output.

**Acceptance Scenarios**:

1. **Given** a fresh checkout, **When** the developer has `rustup` installed, **Then** the correct Rust toolchain version is automatically selected based on the pinned configuration.
2. **Given** any crate in the workspace, **When** the formatter runs, **Then** output matches the project's formatting configuration.
3. **Given** any crate in the workspace, **When** the linter runs, **Then** it enforces the project's zero-warnings policy.

---

### Edge Cases

- What happens when a developer builds a single crate in isolation (e.g., targeting a single crate by name) — does it resolve workspace dependencies correctly?
- How does the workspace handle a contributor using a Rust version older than the MSRV — does it produce a clear error message?
- What happens when the `builtin-tools` feature is disabled on the core crate — do crates that depend on core still compile?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Workspace MUST define all seven crates as members: core library, adapters, memory, local-llm, eval, TUI (binary), and xtask.
- **FR-002**: Workspace MUST centralize shared dependency versions so that all crates resolve common libraries to the same version.
- **FR-003**: Core crate MUST provide a `builtin-tools` feature flag that is enabled by default and gates inclusion of built-in tool implementations.
- **FR-004**: Each library crate MUST have a root module that serves as the public API surface via re-exports.
- **FR-005**: The TUI crate MUST be a binary crate with an application entry point.
- **FR-006**: The workspace MUST enforce a minimum supported Rust version and language edition via configuration.
- **FR-007**: The workspace MUST include linter configuration that treats all warnings as errors.
- **FR-008**: The workspace MUST include formatter configuration for consistent code style.
- **FR-009**: The workspace MUST include a toolchain configuration file that pins the Rust version for automatic selection.
- **FR-010**: Inter-crate dependencies MUST follow the defined dependency chain: adapters and memory depend on core; local-llm depends on core; eval depends on core; TUI depends on core, adapters, memory, and local-llm. No reverse dependencies.
- **FR-011**: The core crate MUST forbid unsafe code at the crate root.
- **FR-012**: All library crates MUST produce no business logic in this scaffold — only structural definitions and empty or stub re-exports.

### Key Entities

- **Workspace**: The root container defining all crate members, shared settings, and centralized dependency versions.
- **Core Crate**: The foundational library that all other crates depend on. Defines the public API surface.
- **Adapters Crate**: Provider-specific implementations, depends only on core.
- **Memory Crate**: Session persistence, depends only on core.
- **Local LLM Crate**: On-device inference, depends only on core.
- **Eval Crate**: Evaluation framework, depends only on core.
- **TUI Crate**: Terminal UI binary, depends on core, adapters, memory, and local-llm.
- **Xtask Crate**: Developer workflow commands, workspace member with no production dependencies.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A single workspace-wide build command completes successfully with zero errors across all seven crates.
- **SC-002**: A single workspace-wide lint check completes with zero warnings under the strict linting policy.
- **SC-003**: The core crate compiles with default features enabled and with all default features disabled.
- **SC-004**: Each crate can be built individually (e.g., targeting a single crate by name) without errors.
- **SC-005**: No crate in the workspace depends on a crate it should not, as defined by the dependency chain.
- **SC-006**: A downstream project depending only on the core crate does not transitively pull in provider, UI, or evaluation dependencies.
- **SC-007**: The toolchain configuration file, when present, causes the correct Rust version to be automatically selected.
- **SC-008**: The formatter produces identical output across different contributor environments when run against the same source.

## Assumptions

- The MSRV is 1.88 with Rust edition 2024, as defined by the project requirements.
- The xtask crate follows the standard Rust xtask convention and does not need to be published.
- Zero-warnings policy is enforced via linter configuration, not just by convention.
- The workspace uses centralized dependency management for shared dependency versions.
