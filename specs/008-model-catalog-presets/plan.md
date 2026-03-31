# Implementation Plan: Model Catalog, Presets & Fallback

**Branch**: `008-model-catalog-presets` | **Date**: 2026-03-20 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/008-model-catalog-presets/spec.md`

## Summary

Provide a data-driven model and provider registry, preset-based connection configuration, automatic model fallback, model cost calculation from catalog pricing data, and capability introspection. The model catalog is loaded from a TOML data file embedded at compile time via `include_str!`, containing provider metadata (credential environment variables, base URLs, auth modes) and model presets grouped by provider and family. Presets carry identifiers, display names, groups, model IDs, capabilities, status, and optional context window sizes. `ModelConnection` and `ModelConnections` resolve presets to fully configured connection objects carrying a `ModelSpec` and `StreamFn`. `ModelFallback` defines an ordered chain of fallback models with automatic failover when the primary model exhausts its retry budget.

## Technical Context

**Language/Version**: Rust 1.88, edition 2024
**Primary Dependencies**: `serde` (deserialization), `toml` (catalog parsing), `tokio` (async runtime), `tokio-util` (CancellationToken), `futures` (stream primitives)
**Storage**: Embedded TOML file (`src/model_catalog.toml`) compiled into the binary via `include_str!`
**Testing**: `cargo test --workspace`
**Target Platform**: Cross-platform library crate
**Project Type**: Library
**Performance Goals**: Catalog loaded once via `OnceLock` (zero-cost after first access); no runtime file I/O
**Constraints**: `#[forbid(unsafe_code)]`, no provider-specific or UI-specific dependencies in core
**Scale/Scope**: 3 source files + 1 embedded data file

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|---|---|---|
| I. Library-First | PASS | All components live in the `swink-agent` core crate as public API surface. No new crates introduced. The catalog is embedded data, not a service dependency. `ModelConnection`, `ModelConnections`, and `ModelFallback` are self-contained types with no UI or provider-specific imports. |
| II. Test-Driven Development | PASS | Each source file includes inline `#[cfg(test)]` modules. Tests cover catalog loading, preset lookup, provider metadata propagation, connection deduplication, fallback chain semantics, capability conversion, cost calculation (known/unknown/zero/cache), and capability introspection (flags, limits, defaults). |
| III. Efficiency & Performance | PASS | Catalog parsed once via `OnceLock` and stored as `&'static ModelCatalog`. No runtime file I/O. `CatalogPreset` flattens provider+preset data to avoid repeated lookups. `ModelConnections` deduplicates extras in `new()` to avoid redundant fallback attempts. |
| IV. Leverage the Ecosystem | PASS | Uses `toml` crate for deserialization (high download count, stable API). Uses `serde::Deserialize` for all catalog types. No hand-rolled parser. |
| V. Provider Agnosticism | PASS | The catalog stores metadata (env var names, base URLs) but never holds API keys or SDK clients. `ModelConnection` pairs a `ModelSpec` with a `StreamFn` — the core crate does not know how to construct provider-specific stream functions. |
| VI. Safety & Correctness | PASS | `#[forbid(unsafe_code)]` enforced. Malformed TOML panics at initialization with a clear message (build-time invariant, not runtime error). Poisoned mutexes not applicable (uses `OnceLock`). |

## Project Structure

### Documentation (this feature)

```text
specs/008-model-catalog-presets/
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
├── model_catalog.rs         # ModelCatalog, ProviderCatalog, PresetCatalog, CatalogPreset,
│                            # enums (ProviderKind, AuthMode, ApiVersion, PresetCapability,
│                            # PresetStatus), model_catalog() singleton
├── model_catalog.toml       # Embedded TOML data file — provider and preset definitions
├── model_presets.rs         # ModelConnection, ModelConnections (primary + extras with dedup)
├── fallback.rs              # ModelFallback (ordered chain of ModelSpec + StreamFn pairs)
└── lib.rs                   # Public re-exports for all catalog, connection, and fallback types

Cargo.toml                   # Workspace deps: toml, serde
```

**Structure Decision**: All model catalog, connection, and fallback types are in the core crate (`swink-agent`). Each concern has its own file: catalog loading and querying (`model_catalog.rs`), connection resolution (`model_presets.rs`), and fallback configuration (`fallback.rs`). The TOML data file sits alongside its consumer in `src/`. Re-exports in `lib.rs` ensure consumers never reach into submodules.

## Complexity Tracking

No constitution violations. All components fit within existing crate boundaries.
