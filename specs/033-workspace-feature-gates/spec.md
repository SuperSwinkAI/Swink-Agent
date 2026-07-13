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

### User Story 2 - Direct Sub-Crate Feature Selection (Priority: P2)

A downstream crate depends on the workspace crates directly and wants to select specific adapters without compiling unrelated providers. The developer writes `swink-agent-adapters = { path = "../Swink-Agent/adapters", features = ["anthropic", "openai"] }` and only those adapter modules compile.

**Why this priority**: Adapter and local-LLM crates are the actual feature-gated compilation boundaries. Clear documentation is still necessary so consumers use the supported dependency surface instead of assuming the root crate forwards features.

**Independent Test**: Can be tested by creating a minimal crate that depends on `swink-agent-adapters` with specific adapter features and verifying it compiles without pulling unwanted providers.

**Acceptance Scenarios**:

1. **Given** a consumer depends on `swink-agent-adapters` with `features = ["anthropic"]`, **When** the consumer builds, **Then** only the Anthropic adapter compiles from the adapters crate.
2. **Given** a consumer depends on `swink-agent-adapters` with no adapter features, **When** the consumer builds, **Then** no provider modules compile and only shared infrastructure remains available.
3. **Given** a consumer depends on `swink-agent-adapters` with `features = ["full"]`, **When** the consumer builds, **Then** all implemented adapter modules compile.

---

### User Story 3 - Local LLM Backend Selection (Priority: P2)

A macOS developer building a desktop app wants local inference via Metal acceleration. They enable the `local-llm` and `metal` features. The build pulls the llama.cpp Metal backend without CUDA dependencies. Conversely, a Windows developer enables `local-llm` and `cuda` for GPU-accelerated inference.

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
- What happens when a future adapter (e.g., 016-azure) is implemented? It follows the established pattern: add a feature flag to the adapters crate, gate the module, and expose it through the adapters crate directly.
- What happens when existing tests run with default features? Workspace tests continue to pass, but the adapters crate now uses explicit opt-in defaults (`default = []`, `full = ["all"]`) instead of enabling every provider implicitly.

## Clarifications

### Session 2026-03-25

- Q: Should the `all` feature include flags for the 4 stub adapters (azure, bedrock, mistral, xai) so they compile under the full-adapter profile? → A: Yes — include stub flags in `all`. Stubs get their own feature flags and `full` enables the all-adapters profile.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The adapters crate MUST expose individual feature flags for all 9 adapter modules — implemented (`anthropic`, `openai`, `ollama`, `gemini`, `proxy`) and stubs (`azure`, `bedrock`, `mistral`, `xai`) — that gate each provider's module compilation and re-exports. Note: the `gemini` feature gates the `google` module (which exports `GeminiStreamFn`); the feature name matches the public type and user mental model. **Addendum (2026-07-06)**: this 9-flag inventory omits two other real, non-provider features verified in `adapters/Cargo.toml`: `openai-compat` (the shared OpenAI-compatible-endpoint machinery that `openai` and `xai` both pull in via `openai = ["openai-compat"]` / `xai = ["openai-compat"]`) and the hidden `__no_default_features_sentinel` feature-leak-detection flag (a footnote-level implementation detail, not consumer-facing). Separately, `swink-agent-tui` exposes its own `adapters` feature (`adapters = ["dep:swink-agent-adapters"]`, with a fixed inner feature set) — this spec's FR-001 only covers the `swink-agent-adapters` crate's own flags, not this downstream re-gating in TUI.
- **FR-002**: The adapters crate MUST compile shared infrastructure (base HTTP client, SSE parsing, error types, conversion utilities) unconditionally, regardless of which provider features are enabled.
- **FR-003**: The adapters crate MUST provide an `all` feature that enables all 9 adapter feature flags (5 implemented + 4 stubs), plus a `full` convenience feature for the all-adapters profile.
- **FR-004**: The local-llm crate MUST expose backend feature flags (`metal`, `cuda`, `cudnn`, `vulkan`) that forward to the corresponding llama-cpp-2 compile-time features. **Corrected 2026-07-06**: this list previously omitted `cudnn` (verified against `local-llm/Cargo.toml`: `cudnn = ["cuda"]`).
- **FR-005**: The local-llm crate MUST default to CPU-only inference when no backend feature is explicitly selected (no `default` or `all` feature for backends — explicit opt-in only).
- **FR-006**: The TUI crate MUST remain opt-in from the workspace root — it MUST NOT be included in the root crate's default features.
- **FR-007**: The TUI crate MUST preserve its existing `local` feature that optionally depends on `swink-agent-local-llm`.
- **FR-008**: ~~The root `swink-agent` crate MUST forward adapter feature flags to the adapters sub-crate so consumers can select providers via the root dependency.~~ **Not feasible**: cyclic dependency (root → adapters → root). Consumers depend on `swink-agent-adapters` directly with feature flags.
- **FR-009**: ~~The root crate MUST expose `adapters-all`, `tui`, and `local-llm` features for coarse-grained control.~~ **Not feasible**: see FR-008. Consumers use sub-crate features directly.
- **FR-010**: The root crate's `default` features MUST include `builtin-tools` (preserving current behavior). Adapters, TUI, and local-llm are separate workspace crates — consumers opt-in by adding them as direct dependencies.
- **FR-011**: Feature-gated modules MUST produce compile-time errors when a consumer references a type whose feature is not enabled (Rust's default "unresolved import" error is sufficient — no explicit `compile_error!` macro required).
- **FR-012**: All existing tests MUST pass with default features enabled, preserving full backward compatibility.
- **FR-013**: Provider-specific dependencies (e.g., the `aws-credential-types`/`aws-sigv4`/`aws-smithy-eventstream`/`aws-smithy-runtime-api`/`aws-smithy-types` family for the bedrock adapter) MUST only compile when the corresponding provider feature is enabled. The shared `sse` module has no external dependencies and compiles unconditionally. **Corrected 2026-07-06**: the original wording claimed `eventsource-stream` gates the proxy adapter and `sha2` gates bedrock — neither dependency exists in `adapters/Cargo.toml`. The proxy adapter's SSE handling is a hand-rolled, always-on module (no optional dependency of its own); bedrock's real optional deps are the `aws-*` family listed above.

### Key Entities

- **Feature Flag**: A Cargo feature marker that gates compilation of a module and its dependencies. Adapters use `default = []` plus `full = ["all"]`; the policies crate keeps its own `default = ["all"]` profile.
- **Shared Infrastructure**: The always-compiled foundation of the adapters crate (base HTTP client, SSE utilities, error types, conversion functions) that all providers depend on.
- **Backend Feature**: A local-llm feature flag (`metal`, `cuda`, `cudnn`, `vulkan`) that selects optional hardware acceleration for on-device inference. CPU-only inference is the no-feature fallback. **Corrected 2026-07-06**: earlier drafts listed phantom `flash-attn`, `mkl`, and `accelerate` flags that don't exist in `local-llm/Cargo.toml`, and omitted the real `vulkan` flag.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A consumer enabling only one adapter feature compiles fewer total crate dependencies than one enabling all adapters (measurable via dependency tree count).
- **SC-002**: All existing workspace tests pass with default features enabled — zero regressions.
- **SC-003**: Building the adapters crate with `--no-default-features --features anthropic` succeeds and excludes all other provider modules from compilation.
- **SC-004**: Building the root crate with `default-features = false` succeeds with only the core agent loop available.
- **SC-005**: A consumer referencing an ungated adapter type receives a compile error indicating the type is unavailable (Rust's "unresolved import" error).
- **SC-006**: The local-llm crate builds with each supported backend feature (`metal`, `cuda`, `cudnn`, `vulkan`) independently on its target platform, and also builds with no backend feature selected. **Corrected 2026-07-06**: see Key Entities addendum above — the phantom `flash-attn`/`mkl`/`accelerate` flags don't exist; `vulkan` does.
- **SC-007**: The TUI crate and its dependencies do not appear in the dependency tree of a consumer that depends only on `swink-agent` (TUI is a separate workspace crate, not a root dependency).

## Assumptions

- The existing `swink-agent-policies` crate feature pattern (individual marker flags plus an `all` convenience feature) is the established convention, but not every crate uses the same default-feature policy.
- Provider-specific dependencies can be cleanly separated — each adapter module's dependencies are identifiable and do not overlap with other providers beyond shared infrastructure.
- The `llama-cpp-2` crate exposes the backend feature flags needed by `swink-agent-local-llm` (`metal`, `cuda`, `vulkan`), while CPU-only inference remains the no-feature fallback.
- The workspace `members` list in root `Cargo.toml` will continue to list all crates, but consumers compile optional functionality by depending on the relevant sub-crates directly.
- Future adapters (016-azure, 017-xai, 018-mistral, 019-bedrock) are not yet implemented; this spec only establishes the pattern they will follow. Existing stub modules for these adapters will be feature-gated alongside implemented ones.
