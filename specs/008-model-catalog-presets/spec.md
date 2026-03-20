# Feature Specification: Model Catalog, Presets & Fallback

**Feature Branch**: `008-model-catalog-presets`
**Created**: 2026-03-20
**Status**: Draft
**Input**: Data-driven model and provider registry, preset-based connection configuration, and automatic model fallback. References: HLD Catalogs & Registries (ModelCatalog, PresetCatalog, ProviderCatalog), HLD Design Decisions (catalogs are core concerns), Provider Expansion Roadmap (catalog shape guidance).

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

### Edge Cases

- What happens when the catalog data file is malformed — does loading fail gracefully with a clear error?
- How does the system handle a provider that has presets but no credential environment variable is set?
- What happens when a fallback chain contains only one model — does it behave identically to no fallback?
- How does the catalog handle duplicate preset identifiers across providers?

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

### Key Entities

- **ModelCatalog**: Registry of providers and presets loaded from a data file. Queryable by provider, group, and capability.
- **Provider**: Metadata for an LLM provider — credential env var, base URL env var, default base URL.
- **Preset**: Metadata for a specific model — ID, display name, group, model ID, capabilities, status, context window.
- **ModelConnection**: Fully resolved connection configuration — base URL, credentials, model specification.
- **ModelFallback**: Ordered chain of model specifications with automatic failover.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The catalog loads successfully and contains all registered providers and presets.
- **SC-002**: Presets can be queried by provider, group, and capability with correct results.
- **SC-003**: Preset-to-connection resolution produces correct credentials and base URLs from environment variables.
- **SC-004**: Automatic fallback tries the next model when the primary fails and stops at the first success.
- **SC-005**: New providers can be added to the catalog data file without modifying code.

## Assumptions

- The catalog data file is embedded in the library at compile time, not loaded from the filesystem at runtime.
- The catalog format is TOML, but the spec does not prescribe the specific format — only the required fields.
- Preset capabilities include at minimum: text generation, tool calling, and reasoning/thinking.
- Preset status indicates whether the model is generally available, in preview, or deprecated.
- Fallback is triggered by errors from the provider, not by response quality.
