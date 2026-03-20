# Research: Adapter Shared Infrastructure

**Feature**: 011-adapter-shared-infra | **Date**: 2026-03-20

## Decision 1: Message Conversion Trait Design

**Question**: How should the conversion trait be structured to handle all message types across 9 adapters?

**Decision**: Define `MessageConverter` as a trait in core (`swink-agent::convert`) with methods for each conversion step: system prompt extraction, message-level conversion, and content-block-level conversion. Each adapter implements the trait for its provider's JSON shape. A generic `convert_messages<C: MessageConverter>()` function drives the loop.

**Rationale**: Defining the trait in core ensures all adapters share the same contract without a circular dependency. The generic function in core eliminates boilerplate — adapters only implement the per-block mapping. Anthropic is the exception: it has a custom `convert_messages` because its system prompt is top-level (not a message) and thinking blocks are filtered.

**Alternatives rejected**:
- *Trait in adapters crate*: Would require core to depend on adapters, violating the dependency chain.
- *Free functions per adapter*: Duplicates the iteration/accumulation logic 9 times.
- *Serde-based auto-conversion*: Provider formats diverge too much for a single derive to work.

## Decision 2: HTTP Error Classification

**Question**: Should error classification be a trait or a utility function?

**Decision**: A utility function (`classify_http_status`) with a companion `classify_with_overrides` for provider-specific tweaks. The function is `const fn` for zero runtime cost.

**Rationale**: All adapters use the same classification logic (429 = throttle, 401/403 = auth, 5xx = network). A trait would add unnecessary indirection for what is a pure function of a status code. The override mechanism (`classify_with_overrides`) handles edge cases (e.g., a provider returning 529 for throttling) without subclassing.

**Alternatives rejected**:
- *Trait with per-adapter impl*: Over-engineered for a status-code lookup table.
- *Match in each adapter*: Duplicates logic; inconsistency risk across adapters.

## Decision 3: SSE Stream Parsing

**Question**: Should SSE parsing use an existing crate or a custom parser?

**Decision**: Custom `SseStreamParser` that buffers raw bytes and yields `SseLine` variants. A higher-level `sse_data_lines()` stream combinator filters to `Data` and `Done` events.

**Rationale**: The SSE protocol is simple (prefix-based line parsing). Existing crates (`eventsource-stream`, `reqwest-eventsource`) add transitive dependencies and don't expose the low-level control adapters need (e.g., Anthropic's `event:` labels drive state-machine transitions). The custom parser is ~100 lines, well-tested, and zero-dependency beyond `futures` and `bytes`.

**Alternatives rejected**:
- *`eventsource-stream` crate*: Adds a dependency for ~100 lines of code; doesn't expose event-type labels cleanly.
- *Per-adapter SSE parsing*: Would duplicate identical line-splitting logic 7 times.

## Decision 4: Catalog-Driven Remote Presets

**Question**: How should the preset system resolve a model selection to a configured `StreamFn`?

**Decision**: `RemotePresetKey` is a `(provider_key, preset_id)` pair. `build_remote_connection(key)` looks up the catalog preset, resolves credentials from environment variables, and constructs the appropriate `StreamFn` behind `Arc<dyn StreamFn>`. The `remote_preset_keys` module provides compile-time constants for every known preset.

**Rationale**: Compile-time keys prevent typos. Environment-variable-based credential resolution works for CLI, TUI, and daemon contexts. The match on `provider_key` is a single dispatch point; adding a provider means adding one arm and the corresponding `StreamFn` import.

**Alternatives rejected**:
- *Registry trait with dynamic registration*: Over-engineered; the provider set is known at compile time.
- *Config-file-based presets*: Adds I/O to what should be a pure construction step; credentials still come from env vars.
