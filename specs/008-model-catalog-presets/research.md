# Research: Model Catalog, Presets & Fallback

**Feature**: 008-model-catalog-presets | **Date**: 2026-03-20

## Design Decisions

### D1: TOML as the Catalog Data Format

**Decision**: Use TOML for the embedded model catalog data file, deserialized via the `toml` crate and `serde::Deserialize`.

**Rationale**: TOML is human-readable, has excellent Rust ecosystem support via the `toml` crate, and maps naturally to the hierarchical provider-preset structure. The `[[providers]]` array-of-tables syntax allows adding new providers by appending sections without modifying existing entries. The `toml` crate is widely used (millions of downloads), well-maintained, and produces clear error messages on parse failure.

**Alternatives rejected**:
- **JSON**: More verbose, no comments, less readable for configuration data. JSON does not support trailing commas or multiline strings cleanly.
- **YAML**: Whitespace-sensitive, more footguns (implicit type coercion). The Rust `serde_yaml` ecosystem is less mature than `toml`.
- **Rust code (const data)**: Requires recompilation for any change to catalog data. Mixing data with code reduces readability and makes non-developer contributions harder.

---

### D2: Compile-Time Embedding via `include_str!`

**Decision**: Embed the catalog TOML file at compile time using `include_str!("model_catalog.toml")`, parsed once via `OnceLock` into a `&'static ModelCatalog`.

**Rationale**: Embedding guarantees the catalog is always available without runtime file I/O, filesystem path resolution, or error handling for missing files. `OnceLock` ensures the TOML is parsed exactly once and subsequent accesses are a pointer dereference. If the TOML is malformed, the panic occurs at first access with a clear error message — this is effectively a build-time invariant since any test that touches the catalog will catch it.

**Alternatives rejected**:
- **Runtime file loading**: Requires error handling for missing/unreadable files. Breaks the library-first principle (no filesystem dependency).
- **Build script (`build.rs`) code generation**: Adds build complexity. `include_str!` is simpler and achieves the same result.
- **Lazy static macro**: `OnceLock` is the standard library solution (stabilized in Rust 1.70), no external dependency needed.

---

### D3: CatalogPreset as a Flattened Denormalized View

**Decision**: `CatalogPreset` is a standalone struct that flattens provider-level fields (credential env var, base URL, auth mode) together with preset-level fields (model ID, capabilities, status) into a single value.

**Rationale**: Callers querying a preset almost always need the provider context (to resolve credentials, determine auth mode, construct connections). Flattening avoids forcing callers to hold references to both the provider and preset, simplifies lifetime management, and makes `CatalogPreset` a self-contained unit that can be passed around without borrowing the catalog. The cost is some string cloning during construction, which is negligible since preset lookups are infrequent operations (configuration time, not hot path).

**Alternatives rejected**:
- **Returning `(&ProviderCatalog, &PresetCatalog)` tuple**: Forces callers to manage two borrows. Awkward API surface.
- **Trait-based access**: Over-engineering for a simple data lookup.

---

### D4: ModelConnection as a ModelSpec + StreamFn Pair

**Decision**: `ModelConnection` pairs a `ModelSpec` with an `Arc<dyn StreamFn>`. It does not hold credentials, base URLs, or provider-specific configuration directly.

**Rationale**: The core crate is provider-agnostic (Constitution Principle V). `ModelConnection` is the resolved result — the `StreamFn` already encapsulates all provider-specific details (credentials, endpoints, SDK clients). The core crate only needs to know *which model* (`ModelSpec`) and *how to stream* (`StreamFn`). Resolution from catalog presets to connections happens in the adapters crate or application layer, not in core.

**Alternatives rejected**:
- **Carrying credentials in ModelConnection**: Violates provider agnosticism. The core crate should not handle API keys.
- **Lazy resolution (resolve on first use)**: Adds complexity and deferred error handling. Fail-fast at configuration time is preferable.

---

### D5: ModelConnections with Deduplication

**Decision**: `ModelConnections` holds a primary `ModelSpec`+`StreamFn` and a deduplicated list of extras. Duplicates of the primary and duplicates among extras are silently dropped during construction.

**Rationale**: Users may configure the same model as both primary and extra (e.g., copy-paste error, or programmatic generation of connection lists). Silently deduplicating is the least surprising behavior — attempting the same model twice in a fallback scenario wastes time and produces the same error. Deduplication uses `ModelSpec` equality (provider + model_id), which is the natural identity for a model connection.

**Alternatives rejected**:
- **Error on duplicate**: Too strict for a configuration convenience type. Users would need to manually deduplicate.
- **Allow duplicates**: Wasteful in fallback scenarios where each model gets a full retry budget.

---

### D6: ModelFallback as an Ordered Vec of (ModelSpec, StreamFn)

**Decision**: `ModelFallback` stores an ordered `Vec<(ModelSpec, Arc<dyn StreamFn>)>`. The agent loop tries each in order, applying the configured `RetryStrategy` independently per model. When all are exhausted, the last error propagates.

**Rationale**: A simple ordered list is the most intuitive fallback model. Each entry is independent — different providers, different models, different stream functions. The agent loop owns the retry logic; `ModelFallback` is purely configuration. An empty fallback chain means no fallback (single-model behavior), which is the zero-cost default.

**Alternatives rejected**:
- **Tree/graph-based fallback**: Over-engineering. Linear fallback covers all practical use cases.
- **Fallback with conditions (e.g., only on rate limit)**: The retry strategy already handles error classification. Adding conditions to fallback would duplicate that logic.
- **Embedding fallback in ModelConnections**: Separating fallback from the connection list keeps each concern focused. `ModelConnections` is about "which models are available"; `ModelFallback` is about "what to do when one fails."

---

### D7: Capability and Status as Enums with Serde Rename

**Decision**: `PresetCapability` and `PresetStatus` are Rust enums with `#[serde(rename_all = "snake_case")]`, mapping directly to TOML string values like `"text"`, `"tools"`, `"thinking"`, `"ga"`, `"preview"`.

**Rationale**: Enums provide exhaustive matching and compile-time safety. `serde` rename ensures the TOML file uses human-readable lowercase strings while Rust code uses idiomatic PascalCase variants. Adding a new capability or status requires adding an enum variant, which forces all match arms to be updated — preventing silent omissions.

**Alternatives rejected**:
- **String-based capabilities**: No compile-time safety. Typos in TOML silently fail.
- **Bitflags**: Less readable in TOML (numeric values). The number of capabilities is small enough that a `Vec<PresetCapability>` is fine.

---

### D8: Pricing Data Inline in Catalog TOML

**Decision**: Add pricing fields (`cost_per_million_input`, `cost_per_million_output`, `cost_per_million_cache_read`, `cost_per_million_cache_write`) as optional `f64` fields directly on `PresetCatalog`. The `calculate_cost()` function looks up pricing by `model_id` across all providers.

**Rationale**: Pricing is per-model, not per-provider — different tiers from the same provider have different prices. Keeping pricing inline with the preset definition ensures pricing and model metadata stay in sync. The fields are `Option<f64>` so models without pricing data (e.g., local models, preview models) simply omit them. A separate pricing file was considered but adds indirection without benefit — pricing changes at the same cadence as model additions.

**Key reference**: Pi Agent's `calculateCost(model, usage)` with pricing baked into model metadata. AWS Bedrock SDK bakes pricing into model definitions.

**Alternatives rejected**:
- **Separate pricing TOML file**: Adds a second file to maintain. Pricing and model metadata change together, so co-location is cleaner.
- **Runtime pricing lookup (API call)**: Violates library-first principle. Adds network dependency for a simple calculation.
- **Pricing as a trait/callback**: Over-engineering. A pure function with catalog lookup is simpler and covers all use cases.

---

### D9: calculate_cost() Lookup by model_id, Not Provider+Preset

**Decision**: `calculate_cost(model_id: &str, usage: &Usage) -> Cost` searches the catalog by `model_id` across all providers, not by `(provider_key, preset_id)`.

**Rationale**: Callers typically have a `ModelSpec` (which carries `model_id`) but not necessarily the preset ID. Searching by `model_id` is the most ergonomic API. Since `model_id` is unique across the catalog (each model ID maps to exactly one preset), the search is unambiguous. Graceful degradation (zero cost) when the model is not found avoids forcing callers to handle errors for what is essentially a monitoring convenience.

**Alternatives rejected**:
- **Lookup by `(provider_key, preset_id)`**: Requires callers to know the preset ID, which is a catalog-internal concept. `model_id` is what the LLM API uses.
- **Lookup by `&ModelSpec`**: Would work but ties the function signature to `ModelSpec`. Using `&str` for `model_id` is more flexible.
- **Returning `Result<Cost, _>`**: Cost calculation is a best-effort convenience. Returning an error for unknown models adds error handling burden with no benefit — zero cost is the correct answer for "I don't know this model's pricing."

---

### D10: ModelCapabilities as Optional Field on ModelSpec

**Decision**: `ModelSpec` carries `capabilities: Option<ModelCapabilities>`. The `capabilities()` method returns stored capabilities or `ModelCapabilities::default()` (all flags false, no limits). `CatalogPreset::model_spec()` pre-populates capabilities from the catalog.

**Rationale**: Capabilities are metadata that flows with the model specification. Making them `Option` preserves backward compatibility — manually created `ModelSpec` instances work without specifying capabilities. The `capabilities()` accessor returning defaults for `None` means callers never need to handle the absence case. This pattern mirrors how `serde(default)` works — absent data gets safe defaults.
