# Tasks: Model Catalog, Presets & Fallback

**Input**: Design documents from `/specs/008-model-catalog-presets/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/public-api.md

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Workspace dependency wiring and file scaffolding

- [x] T001 Add `toml` and `serde` workspace dependencies to `Cargo.toml` if not already present
- [x] T002 [P] Create empty source files `src/model_catalog.rs`, `src/model_presets.rs`, `src/fallback.rs` if not already present
- [x] T003 [P] Create the embedded TOML data file `src/model_catalog.toml` with the initial provider/preset schema structure (empty `[[providers]]` array)
- [x] T004 Add `mod model_catalog;`, `mod model_presets;`, `mod fallback;` declarations and public re-exports to `src/lib.rs`

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Enum types and data structures that all user stories depend on

**CRITICAL**: No user story work can begin until this phase is complete

- [x] T005 [P] Implement enums `ProviderKind`, `AuthMode`, `ApiVersion`, `PresetCapability`, `PresetStatus` with serde derives and `rename_all = "snake_case"` in `src/model_catalog.rs` per contracts/public-api.md
- [x] T006 [P] Implement `PresetCatalog` struct with all fields (id, display_name, group, model_id, api_version, capabilities, status, context_window_tokens, max_output_tokens, include_by_default, repo_id, filename) and serde derives with `#[serde(default)]` on capabilities, include_by_default in `src/model_catalog.rs`
- [x] T007 Implement `ProviderCatalog` struct with all fields (key, display_name, kind, auth_mode, credential_env_var, base_url_env_var, default_base_url, requires_base_url, region_env_var, presets) and `preset(&self, preset_id) -> Option<&PresetCatalog>` method in `src/model_catalog.rs`
- [x] T008 Implement `ModelCatalog` struct with `providers: Vec<ProviderCatalog>` and `provider(&self, provider_key) -> Option<&ProviderCatalog>` method in `src/model_catalog.rs`

**Checkpoint**: Foundation ready — all catalog structs and enums defined, user story implementation can begin

---

## Phase 3: User Story 1 — Browse Available Models from a Catalog (Priority: P1)

**Goal**: Load the model catalog from embedded TOML data and query providers/presets by key, with flattened CatalogPreset views

**Independent Test**: Load the catalog singleton and verify providers, presets, and metadata are correctly populated; query by provider key and verify correct results

### Tests for User Story 1

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation (Constitution Principle II: TDD)**

- [x] T009 [US1] Write unit tests in `src/model_catalog.rs` `#[cfg(test)]` module: catalog loads successfully, provider lookup by key, preset lookup with flattened fields, model_capabilities conversion, model_spec creation, unknown provider returns None

### Implementation for User Story 1

- [x] T010 [P] [US1] Populate `src/model_catalog.toml` with Anthropic provider (credential_env_var, base_url, auth_mode, presets: opus_46, sonnet_46, haiku_45) per quickstart.md examples
- [x] T011 [P] [US1] Populate `src/model_catalog.toml` with OpenAI provider (credential_env_var, base_url, auth_mode, presets: gpt_4o, gpt_4o_mini, o3, o4_mini) with capabilities and context windows
- [x] T012 [P] [US1] Populate `src/model_catalog.toml` with Google provider (credential_env_var, base_url, auth_mode, presets: gemini_25_pro, gemini_25_flash) with api_version overrides
- [x] T013 [P] [US1] Populate `src/model_catalog.toml` with local provider (kind = "local", presets with repo_id and filename fields for HuggingFace models)
- [x] T014 [US1] Implement `model_catalog()` singleton function using `OnceLock` and `include_str!("model_catalog.toml")` with `toml::from_str` deserialization and panic on malformed TOML in `src/model_catalog.rs`
- [x] T015 [US1] Implement `CatalogPreset` struct with all flattened fields (provider + preset metadata) in `src/model_catalog.rs` per data-model.md
- [x] T016 [US1] Implement `ModelCatalog::preset(&self, provider_key, preset_id) -> Option<CatalogPreset>` that constructs flattened view by combining provider and preset fields in `src/model_catalog.rs`
- [x] T017 [US1] Implement `CatalogPreset::model_capabilities(&self) -> ModelCapabilities` converting `Vec<PresetCapability>` to the existing `ModelCapabilities` struct (mapping Text, Tools, Thinking, ImagesIn to supports_tool_use, supports_vision, supports_thinking, max_context_window) in `src/model_catalog.rs`
- [x] T018 [US1] Implement `CatalogPreset::model_spec(&self) -> ModelSpec` creating a `ModelSpec` with provider, model_id, and capabilities pre-populated in `src/model_catalog.rs`

**Checkpoint**: User Story 1 complete — catalog browsing, preset lookup, and capability conversion all functional

---

## Phase 4: User Story 2 — Model Connection Container Types (Priority: P1)

**Goal**: Provide `ModelConnection` and `ModelConnections` as provider-agnostic container types that pair a `ModelSpec` with a `StreamFn`, with deduplication of extras. Note: actual preset-to-connection resolution (reading env vars, applying defaults) is an adapter-layer concern per Constitution Principle V (Provider Agnosticism) and research.md decision D4.

**Independent Test**: Create ModelConnection instances with mock StreamFn, build ModelConnections with duplicates, verify deduplication and into_parts destructuring

### Tests for User Story 2

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation (Constitution Principle II: TDD)**

- [x] T019 [US2] Write unit tests in `src/model_presets.rs` `#[cfg(test)]` module: ModelConnection creation and accessors, ModelConnections deduplication (extras matching primary dropped, duplicate extras dropped), into_parts destructuring, empty extras list

### Implementation for User Story 2

- [x] T020 [P] [US2] Implement `ModelConnection` struct with `model: ModelSpec` and `stream_fn: Arc<dyn StreamFn>`, plus `new()`, `model_spec()`, `stream_fn()` methods in `src/model_presets.rs`
- [x] T021 [US2] Implement `ModelConnections` struct with `primary_model`, `primary_stream_fn`, `extra_models` fields in `src/model_presets.rs`
- [x] T022 [US2] Implement `ModelConnections::new(primary, extras)` with deduplication logic — drop extras matching primary `ModelSpec` and drop duplicate extras against each other in `src/model_presets.rs`
- [x] T023 [US2] Implement `ModelConnections` accessor methods: `primary_model()`, `primary_stream_fn()`, `extra_models()`, `into_parts()` in `src/model_presets.rs`

**Checkpoint**: User Story 2 complete — connections can be created and deduplicated

---

## Phase 5: User Story 3 — Automatic Model Fallback on Failure (Priority: P2)

**Goal**: Provide `ModelFallback` as an ordered chain of `(ModelSpec, Arc<dyn StreamFn>)` pairs for automatic failover configuration

**Independent Test**: Create a ModelFallback chain, verify ordering, len/is_empty semantics, and Debug output format

### Tests for User Story 3

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation (Constitution Principle II: TDD)**

- [x] T024 [US3] Write unit tests in `src/fallback.rs` `#[cfg(test)]` module: creation with multiple models, len/is_empty on empty and non-empty chains, single-model chain behaves like no fallback, models accessor returns correct order, Debug output format

### Implementation for User Story 3

- [x] T025 [P] [US3] Implement `ModelFallback` struct with `models: Vec<(ModelSpec, Arc<dyn StreamFn>)>` and `new()`, `models()`, `is_empty()`, `len()` methods in `src/fallback.rs`
- [x] T026 [US3] Implement custom `Debug` for `ModelFallback` that displays `"provider:model_id"` for each entry without printing stream functions in `src/fallback.rs`

**Checkpoint**: User Story 3 complete — fallback chain can be configured and inspected

---

## Phase 6: User Story 4 — Calculate Model Cost from Usage (Priority: P2) — I21

**Goal**: Compute monetary cost from token usage using catalog pricing data

**Independent Test**: Call `calculate_cost()` with a known model and usage, verify computed cost matches expected values

### Tests for User Story 4

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation (Constitution Principle II: TDD)**

- [ ] T031 [US4] Write unit tests in `src/model_catalog.rs` `#[cfg(test)]` module: `calculate_cost_known_model` (verify input/output/cache costs match expected), `calculate_cost_unknown_model` (returns zero cost), `calculate_cost_zero_usage` (returns zero cost), `calculate_cost_cache_tokens` (verify cache_read and cache_write costs), `calculate_cost_no_pricing_data` (model exists in catalog but has no pricing fields — e.g., local model — returns zero cost)

### Implementation for User Story 4

- [ ] T032 [US4] Add pricing fields to `PresetCatalog` in `src/model_catalog.rs`: `cost_per_million_input: Option<f64>`, `cost_per_million_output: Option<f64>`, `cost_per_million_cache_read: Option<f64>`, `cost_per_million_cache_write: Option<f64>` with `#[serde(default)]`
- [ ] T033 [US4] Add pricing fields to `CatalogPreset` (flattened view) — propagate from `PresetCatalog` during `ModelCatalog::preset()` construction
- [ ] T034 [US4] Add helper method to `ModelCatalog`: `fn find_preset_by_model_id(&self, model_id: &str) -> Option<CatalogPreset>` — search across all providers
- [ ] T035 [US4] Implement `calculate_cost(model_id: &str, usage: &Usage) -> Cost` in `src/model_catalog.rs` — look up pricing via `find_preset_by_model_id`, compute per-category costs, return `Cost::default()` if not found
- [ ] T036 [US4] Populate pricing data for all Anthropic presets in `src/model_catalog.toml` (opus_46, sonnet_46, haiku_45)
- [ ] T037 [P] [US4] Populate pricing data for all OpenAI presets in `src/model_catalog.toml`
- [ ] T038 [P] [US4] Populate pricing data for all Google, Mistral, xAI presets in `src/model_catalog.toml`
- [ ] T039 [US4] Re-export `calculate_cost` from `src/lib.rs`

**Checkpoint**: Cost calculation functional — monetary cost can be computed from token usage for any cataloged model

---

## Phase 7: User Story 5 — Query Model Capabilities at Runtime (Priority: P2) — I22

**Goal**: Formalize capability introspection from catalog presets through ModelSpec

**Independent Test**: Load a catalog preset, call `model_capabilities()`, verify correct flags and limits

> **NOTE**: `ModelCapabilities`, `CatalogPreset::model_capabilities()`, and `ModelSpec::capabilities()` already exist. This phase validates existing behavior against the new acceptance scenarios and adds any missing test coverage.

### Tests for User Story 5

- [ ] T040 [US5] Write unit tests in `src/model_catalog.rs` `#[cfg(test)]` module (if not already covered): `capabilities_from_catalog_preset` (verify all flags from catalog capabilities), `capabilities_context_window_and_output` (verify max_context_window and max_output_tokens from catalog), `model_spec_carries_capabilities` (verify capabilities survive model_spec() creation), `manual_model_spec_defaults` (verify ModelSpec::new() has default capabilities)

**Checkpoint**: Capability introspection formally verified — catalog capabilities flow correctly through ModelSpec

---

## Phase 8: Polish & Cross-Cutting Concerns

**Purpose**: Re-exports, final validation, and cleanup

- [x] T027 [P] Verify all public types are re-exported from `src/lib.rs` per contracts/public-api.md: ModelFallback, ApiVersion, AuthMode, CatalogPreset, ModelCatalog, PresetCapability, PresetCatalog, PresetStatus, ProviderCatalog, ProviderKind, model_catalog, ModelConnection, ModelConnections
- [x] T028 [P] Run `cargo clippy --workspace -- -D warnings` and fix any warnings
- [x] T029 Run `cargo test --workspace` to verify all tests pass
- [x] T030 Run quickstart.md validation — verify all code examples compile conceptually against the implemented API
- [ ] T041 Verify `calculate_cost` is re-exported from `src/lib.rs`
- [ ] T042 Run `cargo clippy --workspace -- -D warnings` with updated catalog and fix any warnings
- [ ] T043 Run `cargo test --workspace` to verify all new tests pass
- [ ] T044 Validate new quickstart.md examples (cost calculation, capability query) match actual API

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **User Story 1 (Phase 3)**: Depends on Foundational phase completion
- **User Story 2 (Phase 4)**: Depends on Foundational phase completion — can run in parallel with US1 (different files)
- **User Story 3 (Phase 5)**: Depends on Foundational phase completion — can run in parallel with US1 and US2 (different file)
- **User Story 4 — Cost (Phase 6)**: Depends on US1 (needs catalog with presets). Modifies `model_catalog.rs` and `model_catalog.toml`.
- **User Story 5 — Capabilities (Phase 7)**: Depends on US1 (needs catalog). Primarily test verification of existing behavior.
- **Polish (Phase 8)**: Depends on all user stories being complete

### User Story Dependencies

- **User Story 1 (P1)**: Depends on Phase 2 only. Self-contained in `src/model_catalog.rs` and `src/model_catalog.toml`
- **User Story 2 (P1)**: Depends on Phase 2 only. Self-contained in `src/model_presets.rs`. Uses `ModelSpec` and `StreamFn` from existing core types
- **User Story 3 (P2)**: Depends on Phase 2 only. Self-contained in `src/fallback.rs`. Uses `ModelSpec` and `StreamFn` from existing core types
- **User Story 4 (P2)**: Depends on US1 (catalog with presets). Adds pricing fields to `PresetCatalog`/`CatalogPreset` and `calculate_cost()` function in `src/model_catalog.rs`
- **User Story 5 (P2)**: Depends on US1 (catalog). Primarily test-only — validates existing `ModelCapabilities` behavior against formal acceptance scenarios

### Within Each User Story

- Tests FIRST — write failing tests before implementation (Constitution Principle II: TDD)
- Data types before methods
- Methods before singleton/construction logic
- Story complete before moving to Polish phase

### Parallel Opportunities

- T002 and T003 can run in parallel (different files)
- T005 and T006 can run in parallel (different structs, same file but non-overlapping sections)
- T019 can run in parallel with US1 tasks (different file: model_presets.rs vs model_catalog.rs)
- T024 can run in parallel with US1 and US2 tasks (different file: fallback.rs)
- T027 and T028 can run in parallel (different concerns)
- US1, US2, US3 can proceed in parallel after Phase 2 since each lives in a separate source file
- US4 and US5 can proceed in parallel after US1 (both modify model_catalog.rs but US5 is test-only)

---

## Parallel Example: All User Stories

```bash
# After Phase 2 (Foundational) completes, all three user stories can start simultaneously:

# Developer A: User Story 1 — src/model_catalog.rs + src/model_catalog.toml
Task T009-T018

# Developer B: User Story 2 — src/model_presets.rs
Task T019-T023

# Developer C: User Story 3 — src/fallback.rs
Task T024-T026
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (T001-T004)
2. Complete Phase 2: Foundational (T005-T008)
3. Complete Phase 3: User Story 1 (T009-T018)
4. **STOP and VALIDATE**: Load catalog, query providers, verify presets
5. The catalog is independently useful even without connections or fallback

### Incremental Delivery

1. Setup + Foundational → All types defined
2. Add User Story 1 → Catalog browsing works → MVP
3. Add User Story 2 → Connection resolution works → Enhanced
4. Add User Story 3 → Fallback configuration works → Production-ready
5. Add User Story 4 → Cost calculation works → Cost-aware
6. Add User Story 5 → Capabilities formally verified → Introspectable
7. Polish → Clean re-exports, zero warnings, all tests green

### Parallel Team Strategy

With multiple developers after Phase 2:
- Developer A: User Story 1 (model_catalog.rs + model_catalog.toml)
- Developer B: User Story 2 (model_presets.rs)
- Developer C: User Story 3 (fallback.rs)
- Stories complete and integrate independently via lib.rs re-exports

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story is independently completable and testable
- All source files live in `src/` of the `swink-agent` core crate
- The TOML data file (`src/model_catalog.toml`) is embedded at compile time — changes require recompilation but no code changes
- US4 (Cost Calculation) is new work — adds pricing fields to catalog and `calculate_cost()` function
- US5 (Capability Introspection) is primarily test verification of existing behavior — `ModelCapabilities` already exists
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
