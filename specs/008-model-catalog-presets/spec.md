# Feature Specification: Model Catalog, Presets & Fallback

**Feature Branch**: `008-model-catalog-presets`
**Created**: 2026-03-20
**Status**: Draft
**Input**: Data-driven model and provider registry, preset-based connection configuration, automatic model fallback, model cost calculation from catalog pricing data, and capability introspection. References: HLD Catalogs & Registries (ModelCatalog, PresetCatalog, ProviderCatalog), HLD Design Decisions (catalogs are core concerns), Provider Expansion Roadmap (catalog shape guidance).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Browse Available Models from a Catalog (Priority: P1)

A developer queries the model catalog to discover available models and their capabilities. The catalog is loaded from a data file embedded in the library, containing provider metadata (credential environment variables, base URLs) and model presets grouped by provider and family. The developer can filter by provider, capability, or status.

**Why this priority**: The catalog is the single source of truth for model metadata. All provider selection — in the TUI, in applications, in tests — depends on it.

**Independent Test**: Can be tested by loading the catalog and querying for models by provider, verifying correct metadata is returned.

**Acceptance Scenarios**:

1. **Given** the embedded catalog, **When** the developer loads it, **Then** all registered providers and their presets are available.
2. **Given** the catalog, **When** the developer queries by provider name, **Then** only presets for that provider are returned.
3. **Given** a preset, **When** it is inspected, **Then** it carries: identifier, display name, group, model ID, capabilities, status, and optional context window size.

---

### User Story 2 - Resolve a Preset to a Connection (Priority: P1)

A developer selects a model preset and resolves it to a fully configured connection object. The resolution process reads credential environment variables, applies default base URLs, and produces a connection ready to construct a streaming function. This eliminates hardcoded provider configuration.

**Why this priority**: Preset-to-connection resolution is how applications go from "I want to use Claude Sonnet" to a working LLM connection without manual configuration.

**Independent Test**: Can be tested by selecting a preset, setting the required environment variables, and verifying the resolved connection has the correct endpoint and credentials.

**Acceptance Scenarios**:

1. **Given** a preset and the required credential environment variable set, **When** the preset is resolved, **Then** a connection object is produced with the correct base URL and credentials.
2. **Given** a preset with a custom base URL environment variable set, **When** the preset is resolved, **Then** the custom base URL overrides the default.
3. **Given** a preset without the required credentials, **When** resolution is attempted, **Then** it indicates the missing credentials.

---

### User Story 3 - Automatic Model Fallback on Failure (Priority: P2)

A developer configures a primary model with one or more fallback models. When the primary model fails (rate limit, error, unavailability), the system automatically tries the next model in the fallback chain. This enables resilient agent deployments without application-level retry logic.

**Why this priority**: Fallback is important for production resilience but is an enhancement over basic single-model operation.

**Independent Test**: Can be tested by configuring a fallback chain where the primary model fails and verifying the system falls back to the secondary.

**Acceptance Scenarios**:

1. **Given** a fallback chain of [primary, secondary], **When** the primary model succeeds, **Then** the secondary is not attempted.
2. **Given** a fallback chain, **When** the primary model fails with a retryable error, **Then** the system tries the secondary model.
3. **Given** a fallback chain, **When** all models fail, **Then** the last error is surfaced to the caller.

---

### User Story 4 - Calculate Model Cost from Usage (Priority: P2) — I21

A developer calculates the monetary cost of an LLM call by passing a model specification and token usage to a `calculate_cost()` function. The function looks up per-million-token pricing data from the catalog and computes input, output, cache read, cache write, and total costs. If the model is not found in the catalog, zero cost is returned (graceful degradation).

**Why this priority**: Cost visibility is essential for production monitoring and budget enforcement. Without catalog-driven pricing, operators must maintain separate pricing tables or hardcode costs.

**Independent Test**: Can be tested by calling `calculate_cost()` with a known model and usage, and verifying the computed cost matches the expected value from the catalog's pricing data.

**Acceptance Scenarios**:

1. **Given** a model in the catalog with pricing data and a `Usage` with input/output tokens, **When** `calculate_cost()` is called, **Then** the returned `Cost` has correct `input`, `output`, and `total` fields computed as `tokens * price_per_million / 1_000_000`.
2. **Given** a `Usage` with `cache_read` and `cache_write` tokens, **When** `calculate_cost()` is called, **Then** the returned `Cost` includes correct `cache_read` and `cache_write` fields.
3. **Given** a model not in the catalog (unknown model), **When** `calculate_cost()` is called, **Then** a zero `Cost` is returned (all fields are 0.0).
4. **Given** a model with pricing data but zero usage tokens, **When** `calculate_cost()` is called, **Then** a zero `Cost` is returned.

---

### User Story 5 - Query Model Capabilities at Runtime (Priority: P2) — I22

A developer queries a model's capabilities at runtime to determine what features it supports (thinking, tool use, vision, streaming, structured output) and its limits (context window, max output tokens). Capabilities are populated from the catalog when a `ModelSpec` is created via `CatalogPreset::model_spec()`, and are queryable via `ModelSpec::capabilities()`.

**Why this priority**: Capability introspection enables adaptive agent behavior — e.g., only requesting thinking mode from models that support it, or adjusting context size to fit the model's window.

**Independent Test**: Can be tested by loading a catalog preset, calling `model_capabilities()`, and verifying the correct flags and limits are set.

**Acceptance Scenarios**:

1. **Given** a catalog preset with capabilities `["thinking", "tools", "images_in", "streaming"]`, **When** `model_capabilities()` is called, **Then** the returned `ModelCapabilities` has `supports_thinking: true`, `supports_tool_use: true`, `supports_vision: true`, `supports_streaming: true`.
2. **Given** a catalog preset with `context_window_tokens = 200000` and `max_output_tokens = 64000`, **When** `model_capabilities()` is called, **Then** the returned `ModelCapabilities` has `max_context_window: Some(200000)` and `max_output_tokens: Some(64000)`.
3. **Given** a `ModelSpec` created via `CatalogPreset::model_spec()`, **When** `spec.capabilities()` is called, **Then** it returns the same capabilities as the catalog preset.
4. **Given** a `ModelSpec` created manually (not from catalog), **When** `spec.capabilities()` is called, **Then** it returns default capabilities (all flags false, no limits).

---

### Edge Cases

- What happens when the catalog data file is malformed — the catalog is embedded at compile time; malformed TOML panics at initialization with a clear message. This is a build-time invariant, not a runtime error.
- How does the system handle a provider that has presets but no credential environment variable is set — resolution indicates the missing credentials; callers handle it (e.g., TUI shows a setup wizard).
- What happens when a fallback chain contains only one model — behaves identically to no fallback; the single model is tried and errors propagate normally.
- How does the catalog handle duplicate preset identifiers across providers — preset lookup uses first-match via `iter().find()`. Duplicates are prevented by TOML structure (unique keys per provider section).
- What happens when pricing data is missing for a model in the catalog — `calculate_cost()` returns zero cost. Models without pricing fields are treated as free (graceful degradation, not an error).
- What happens when pricing data changes between catalog versions — pricing is compiled into the binary. To update pricing, update `model_catalog.toml` and recompile. Runtime pricing updates are out of scope.
- What happens when `Usage.extra` contains provider-specific token categories — `calculate_cost()` only computes costs for standard categories (input, output, cache_read, cache_write). Provider-specific costs in `Cost.extra` must be computed by the adapter.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST provide a model catalog loaded from an embedded data file containing provider and preset metadata.
- **FR-002**: Each provider in the catalog MUST define: credential environment variable name, base URL environment variable name, default base URL, and a list of presets.
- **FR-003**: Each preset MUST define: unique identifier, display name, group (family), model ID, capabilities, status, and optional context window size.
- **FR-004**: The catalog MUST support querying presets by provider, group, capability, and status.
- **FR-005**: System MUST provide a connection type that carries all information needed to construct a streaming function: base URL, credentials, model specification.
- **FR-006**: System MUST resolve presets to connections using environment variables for credentials and base URL, with catalog-provided defaults.
- **FR-007**: System MUST support automatic model fallback with a configurable chain of model specifications.
- **FR-008**: Fallback MUST try each model in order, stopping at the first success. If all fail, the last error is surfaced.
- **FR-009**: The catalog MUST be extensible — new providers and presets can be added by updating the data file without code changes.
- **FR-010**: The catalog MUST support per-preset pricing fields: `cost_per_million_input`, `cost_per_million_output`, `cost_per_million_cache_read`, `cost_per_million_cache_write` (all `Option<f64>`, defaulting to `None`/zero).
- **FR-011**: System MUST provide a `calculate_cost(model_id: &str, usage: &Usage) -> Cost` function that looks up pricing from the catalog and computes monetary cost from token counts.
- **FR-012**: `calculate_cost()` MUST return zero cost for models not found in the catalog or models without pricing data (graceful degradation, not an error).
- **FR-013**: The catalog MUST expose capability metadata per preset via `CatalogPreset::model_capabilities()`, mapping capability flags and token limits to the `ModelCapabilities` struct.
- **FR-014**: `ModelSpec` MUST carry optional `capabilities: Option<ModelCapabilities>` and provide `capabilities() -> ModelCapabilities` returning stored capabilities or defaults.

### Key Entities

- **ModelCatalog**: Registry of providers and presets loaded from a data file. Queryable by provider, group, and capability.
- **Provider**: Metadata for an LLM provider — credential env var, base URL env var, default base URL.
- **Preset**: Metadata for a specific model — ID, display name, group, model ID, capabilities, status, context window.
- **ModelConnection**: Fully resolved connection configuration — base URL, credentials, model specification.
- **ModelFallback**: Ordered chain of model specifications with automatic failover.
- **ModelPricing**: Per-preset pricing data (cost per million tokens for input, output, cache read, cache write) embedded in the catalog TOML.
- **calculate_cost()**: Free function computing `Cost` from `Usage` using catalog pricing data.
- **ModelCapabilities**: Capability flags and token limits queryable from `ModelSpec` or `CatalogPreset`.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The catalog loads successfully and contains all registered providers and presets.
- **SC-002**: Presets can be queried by provider, group, and capability with correct results.
- **SC-003**: Preset-to-connection resolution produces correct credentials and base URLs from environment variables.
- **SC-004**: Automatic fallback tries the next model when the primary fails and stops at the first success.
- **SC-005**: New providers can be added to the catalog data file without modifying code.
- **SC-006**: `calculate_cost()` correctly computes input, output, cache read, cache write, and total costs from catalog pricing data and token usage.
- **SC-007**: `calculate_cost()` returns zero cost for unknown models or models without pricing data.
- **SC-008**: `CatalogPreset::model_capabilities()` correctly maps catalog capability flags and token limits to `ModelCapabilities`.
- **SC-009**: `ModelSpec::capabilities()` returns catalog-populated capabilities when created from a preset, or defaults when created manually.

## Clarifications

### Session 2026-03-20

- Q: How does malformed catalog data fail? → A: Compile-time embed; panics at init with clear message. Build-time invariant.
- Q: What if provider credentials are missing? → A: Resolution indicates missing credentials; callers handle (e.g., setup wizard).
- Q: Single-model fallback behavior? → A: Identical to no fallback; single model tried, errors propagate.
- Q: Duplicate preset IDs across providers? → A: First-match via find(); TOML structure prevents duplicates per provider.
### Session 2026-03-31

- Q: Should `Eq` be removed from `PresetCatalog` and `CatalogPreset` after adding `f64` pricing fields? → A: Yes — remove `Eq`, keep `PartialEq`. `f64` does not implement `Eq`, but `PartialEq` works fine. NaN pricing values are not a realistic concern since TOML parsing produces finite floats.

### Session 2026-03-20

- Q: How does FR-004 filtering by group/capability/status work? → A: The catalog exposes `providers` and their `presets` as public fields. Callers filter by iterating over presets and matching on group, capability, or status fields directly. The core catalog provides lookup by provider key and preset ID; advanced filtering is a caller concern.
- Q: Does FR-006 (preset-to-connection resolution via env vars) live in core? → A: No. The core crate provides `ModelConnection` as a provider-agnostic container. Actual env-var resolution is an adapter-layer responsibility per Constitution Principle V (Provider Agnosticism).
- Q: Does FR-008 (fallback execution) live in this feature? → A: No. `ModelFallback` is configuration only. The agent loop (spec 004) consumes it to implement failover execution.

## Assumptions

- The catalog data file is embedded in the library at compile time, not loaded from the filesystem at runtime.
- The catalog format is TOML, but the spec does not prescribe the specific format — only the required fields.
- Preset capabilities include at minimum: text generation, tool calling, and reasoning/thinking.
- Preset status indicates whether the model is generally available, in preview, or deprecated.
- Fallback is triggered by errors from the provider, not by response quality.
- Pricing data is per-preset, not per-provider — different model tiers from the same provider have different prices.
- Pricing is in USD per million tokens, consistent across all providers.
- `calculate_cost()` looks up pricing by `model_id` (searching across all providers), not by provider key + preset ID.
- `ModelCapabilities` already exists in `src/types.rs` — this spec formalizes its usage, not its creation.
