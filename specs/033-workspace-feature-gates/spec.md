# Feature Specification: Workspace Feature Gates

**Feature Branch**: `033-workspace-feature-gates`
**Created**: 2026-03-25
**Status**: Draft
**Input**: User description: "Granular feature gating across the swink-agent workspace to let consumers compile only what they need."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Selective Adapter Compilation (Priority: P1)

A downstream product (e.g., SuperSwink-Core) depends on swink-agent but only uses Anthropic and OpenAI providers. The developer adds `swink-agent-adapters` with `features = ["anthropic", "openai"]` and the build excludes Ollama, Gemini, proxy, and all their provider-specific dependencies. Compile times and binary size decrease proportionally.

**Why this priority**: Adapters are the most commonly customized dependency — every consumer knows which providers they need. This delivers the largest practical benefit with the broadest audience.

**Independent Test**: Can be fully tested by building the adapters crate with a single feature flag (e.g., `--no-default-features --features anthropic`) and verifying that only the Anthropic module compiles and other provider types are absent from the binary.

**Acceptance Scenarios**:

1. **Given** a consumer depends on `swink-agent-adapters` with `features = ["anthropic"]`, **When** the consumer builds, **Then** only `AnthropicStreamFn` and shared infrastructure are compiled; `OpenAiStreamFn`, `OllamaStreamFn`, `GeminiStreamFn`, `ProxyStreamFn` are absent.
2. **Given** a consumer depends on `swink-agent-adapters` with default features, **When** the consumer builds, **Then** all implemented providers compile (backward-compatible behavior).
3. **Given** a consumer enables only `ollama`, **When** the consumer attempts to reference `AnthropicStreamFn`, **Then** a clear compile-time error indicates the `anthropic` feature is required.

---

### User Story 2 - Root-Level Feature Forwarding (Priority: P2)

A downstream crate depends on the root `swink-agent` crate and wants to select specific adapters without directly depending on the adapters sub-crate. The developer writes `swink-agent = { path = "../Swink-Agent", features = ["anthropic", "openai"] }` and the features flow through to the adapters crate automatically.

**Why this priority**: Consumers interact with the root crate, not sub-crates directly. Without feature forwarding, the per-adapter gates are inconvenient to use and the ergonomics gap undermines adoption.

**Independent Test**: Can be tested by creating a minimal crate that depends on `swink-agent` with specific adapter features and verifying it compiles without pulling unwanted providers.

**Acceptance Scenarios**:

1. **Given** a consumer depends on root `swink-agent` with `features = ["anthropic"]`, **When** the consumer builds, **Then** only the Anthropic adapter compiles from the adapters crate.
2. **Given** a consumer depends on root `swink-agent` with no adapter features, **When** the consumer builds, **Then** no adapters compile (shared infrastructure is still available).
3. **Given** a consumer depends on root `swink-agent` with `features = ["adapters-all"]`, **When** the consumer builds, **Then** all implemented adapters compile.

---

### User Story 3 - Local LLM Backend Selection (Priority: P2)

A macOS developer building a desktop app wants local inference via Metal acceleration. They enable the `local-llm` and `metal` features. The build pulls the mistral.rs Metal backend without CUDA dependencies. Conversely, a Windows developer enables `local-llm` and `cuda` for GPU-accelerated inference.

**Why this priority**: The local-llm dependency chain is the heaviest in the workspace and platform-sensitive. Incorrect backend selection causes build failures or bloated binaries.

**Independent Test**: Can be tested by building `swink-agent-local-llm` with `--features metal` on macOS and verifying that CUDA-related dependencies are absent from the build.

**Acceptance Scenarios**:

1. **Given** a macOS consumer enables `metal` on the local-llm crate, **When** the consumer builds, **Then** Metal acceleration is available and CUDA dependencies are excluded.
2. **Given** a Windows consumer enables `cuda` on the local-llm crate, **When** the consumer builds, **Then** CUDA acceleration is available and Metal dependencies are excluded.
3. **Given** a consumer enables `local-llm` without a backend feature, **When** the consumer builds, **Then** the CPU fallback backend is used.

---

### User Story 4 - TUI Exclusion for Library Consumers (Priority: P3)

A headless daemon or library consumer depends on `swink-agent` for its agent loop and adapters but does not need terminal UI. By default, the TUI crate is not pulled into their dependency tree. They never see ratatui, crossterm, syntect, arboard, or keyring in their build.

**Why this priority**: TUI exclusion prevents wasted compile time for the majority of consumers who embed the agent in a non-terminal context, but it is lower priority because the TUI is already a separate workspace crate — the risk is accidental transitive inclusion, not unconditional compilation.

**Independent Test**: Can be tested by building `swink-agent` without the `tui` feature and verifying that ratatui/crossterm are absent from the dependency tree output.

**Acceptance Scenarios**:

1. **Given** a consumer depends on root `swink-agent` with default features, **When** the consumer inspects the dependency tree, **Then** TUI dependencies (ratatui, crossterm, syntect, arboard, keyring) are absent.
2. **Given** a consumer explicitly enables the `tui` feature, **When** the consumer builds, **Then** the TUI crate and all its dependencies compile.

---

### Edge Cases

- What happens when a consumer enables zero adapter features? Only shared infrastructure (base HTTP client, SSE parsing, error types) compiles. No provider types are available.
- What happens when a consumer enables conflicting local-llm backends (e.g., both `metal` and `cuda`)? Both backends compile; runtime selection determines which is used. No compile-time conflict.
- What happens when a consumer uses `default-features = false` on the root crate? Only the bare core compiles — no builtin-tools, no adapters, no TUI, no local-llm.
- What happens when a future adapter (e.g., 016-azure) is implemented? It follows the established pattern: add a feature flag to the adapters crate, gate the module, forward the feature from the root crate.
- What happens when existing tests run with default features? All tests pass identically to current behavior since `default = ["all"]` enables everything.

## Clarifications

### Session 2026-03-25

- Q: Should the `all` feature include flags for the 4 stub adapters (azure, bedrock, mistral, xai) so they compile under `default = ["all"]`? → A: Yes — include stub flags in `all`. Stubs get their own feature flags and `all` enables all 9 adapter modules, preserving current behavior.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The adapters crate MUST expose individual feature flags for all 9 adapter modules — implemented (`anthropic`, `openai`, `ollama`, `gemini`, `proxy`) and stubs (`azure`, `bedrock`, `mistral`, `xai`) — that gate each provider's module compilation and re-exports.
- **FR-002**: The adapters crate MUST compile shared infrastructure (base HTTP client, SSE parsing, error types, conversion utilities) unconditionally, regardless of which provider features are enabled.
- **FR-003**: The adapters crate MUST provide an `all` feature that enables all 9 adapter feature flags (5 implemented + 4 stubs), and `default` MUST include `all`.
- **FR-004**: The local-llm crate MUST expose backend feature flags (`metal`, `cuda`, `cudnn`, `flash-attn`, `mkl`, `accelerate`) that forward to the corresponding mistralrs compile-time features.
- **FR-005**: The local-llm crate MUST default to CPU-only inference when no backend feature is explicitly selected (no `default` or `all` feature for backends — explicit opt-in only).
- **FR-006**: The TUI crate MUST remain opt-in from the workspace root — it MUST NOT be included in the root crate's default features.
- **FR-007**: The TUI crate MUST preserve its existing `local` feature that optionally depends on `swink-agent-local-llm`.
- **FR-008**: The root `swink-agent` crate MUST forward adapter feature flags to the adapters sub-crate so consumers can select providers via the root dependency.
- **FR-009**: The root crate MUST expose `adapters-all`, `tui`, and `local-llm` features for coarse-grained control.
- **FR-010**: The root crate's `default` features MUST include `builtin-tools` (preserving current behavior) but MUST NOT include adapters, TUI, or local-llm by default.
- **FR-011**: Feature-gated modules MUST produce clear compile-time errors when a consumer references a type whose feature is not enabled, not silent omission.
- **FR-012**: All existing tests MUST pass with default features enabled, preserving full backward compatibility.
- **FR-013**: Provider-specific dependencies (e.g., `eventsource-stream` for SSE-based adapters) MUST only compile when the corresponding provider feature is enabled.

### Key Entities

- **Feature Flag**: A Cargo feature marker that gates compilation of a module and its dependencies. Follows the `default = ["all"]` pattern established by the policies crate.
- **Shared Infrastructure**: The always-compiled foundation of the adapters crate (base HTTP client, SSE utilities, error types, conversion functions) that all providers depend on.
- **Backend Feature**: A local-llm feature flag (`metal`, `cuda`, `cpu`) that selects the hardware acceleration strategy for on-device inference.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A consumer enabling only one adapter feature compiles fewer total crate dependencies than one enabling all adapters (measurable via dependency tree count).
- **SC-002**: All existing workspace tests pass with default features enabled — zero regressions.
- **SC-003**: Building the adapters crate with `--no-default-features --features anthropic` succeeds and excludes all other provider modules from compilation.
- **SC-004**: Building the root crate with `default-features = false` succeeds with only the core agent loop available.
- **SC-005**: A consumer referencing an ungated adapter type receives a compile error that names the missing feature flag.
- **SC-006**: The local-llm crate builds with each backend feature (`metal`, `cuda`, `cpu`) independently on its target platform.
- **SC-007**: The TUI crate and its dependencies do not appear in the dependency tree for a consumer that has not enabled the `tui` feature.

## Assumptions

- The existing `swink-agent-policies` crate feature pattern (`default = ["all"]`, individual marker flags, `cfg(feature = "...")` guards) is the established convention and will be followed consistently.
- Provider-specific dependencies can be cleanly separated — each adapter module's dependencies are identifiable and do not overlap with other providers beyond shared infrastructure.
- The `mistral.rs` crate exposes feature flags for Metal, CUDA, and CPU backends that can be forwarded through the local-llm crate's own features.
- The workspace `members` list in root `Cargo.toml` will continue to list all crates, but the root `swink-agent` library crate's optional dependencies gate what consumers actually compile.
- Future adapters (016-azure, 017-xai, 018-mistral, 019-bedrock) are not yet implemented; this spec only establishes the pattern they will follow. Existing stub modules for these adapters will be feature-gated alongside implemented ones.
