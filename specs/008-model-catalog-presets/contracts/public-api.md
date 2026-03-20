# Public API: Model Catalog, Presets & Fallback

**Feature**: 008-model-catalog-presets | **Date**: 2026-03-20

All types are re-exported from `swink_agent` via `lib.rs`. Consumers never reach into submodules.

## `src/model_catalog.rs` — Catalog Types and Singleton

```rust
/// Provider classification.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Remote,
    Local,
}

/// Authentication method for a provider.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    Bearer,
    ApiKeyHeader,
    AwsSigv4,
}

/// API version override for provider-specific endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiVersion {
    V1,
    V1beta,
}

/// Model capability flags.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetCapability {
    Text,
    Tools,
    Thinking,
    ImagesIn,
    Streaming,
    StructuredOutput,
}

/// Model release status.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetStatus {
    Ga,
    Preview,
}

/// A single model preset within a provider.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PresetCatalog {
    pub id: String,
    pub display_name: String,
    pub group: Option<String>,
    pub model_id: String,
    pub api_version: Option<ApiVersion>,
    pub capabilities: Vec<PresetCapability>,       // #[serde(default)]
    pub status: Option<PresetStatus>,
    pub context_window_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub include_by_default: bool,                  // #[serde(default)]
    pub repo_id: Option<String>,
    pub filename: Option<String>,
}

/// Provider metadata and its model presets.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProviderCatalog {
    pub key: String,
    pub display_name: String,
    pub kind: ProviderKind,
    pub auth_mode: Option<AuthMode>,
    pub credential_env_var: Option<String>,
    pub base_url_env_var: Option<String>,
    pub default_base_url: Option<String>,
    pub requires_base_url: bool,                   // #[serde(default)]
    pub region_env_var: Option<String>,
    pub presets: Vec<PresetCatalog>,                // #[serde(default)]
}

impl ProviderCatalog {
    pub fn preset(&self, preset_id: &str) -> Option<&PresetCatalog>;
}

/// Top-level model catalog containing all providers.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ModelCatalog {
    pub providers: Vec<ProviderCatalog>,            // #[serde(default)]
}

impl ModelCatalog {
    pub fn provider(&self, provider_key: &str) -> Option<&ProviderCatalog>;
    pub fn preset(&self, provider_key: &str, preset_id: &str) -> Option<CatalogPreset>;
}

/// Flattened view of a preset with its provider context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogPreset {
    pub provider_key: String,
    pub provider_display_name: String,
    pub provider_kind: ProviderKind,
    pub preset_id: String,
    pub display_name: String,
    pub group: Option<String>,
    pub model_id: String,
    pub api_version: Option<ApiVersion>,
    pub capabilities: Vec<PresetCapability>,
    pub status: Option<PresetStatus>,
    pub context_window_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub auth_mode: Option<AuthMode>,
    pub credential_env_var: Option<String>,
    pub base_url_env_var: Option<String>,
    pub default_base_url: Option<String>,
    pub requires_base_url: bool,
    pub region_env_var: Option<String>,
    pub include_by_default: bool,
    pub repo_id: Option<String>,
    pub filename: Option<String>,
}

impl CatalogPreset {
    /// Convert catalog capabilities to a ModelCapabilities struct.
    pub fn model_capabilities(&self) -> ModelCapabilities;

    /// Create a ModelSpec pre-populated with capabilities from the catalog.
    pub fn model_spec(&self) -> ModelSpec;
}

/// Returns the singleton model catalog loaded from embedded TOML.
/// Panics if the TOML is malformed.
pub fn model_catalog() -> &'static ModelCatalog;
```

## `src/model_presets.rs` — Connection Types

```rust
/// A resolved model connection pairing a ModelSpec with its StreamFn.
#[derive(Clone)]
pub struct ModelConnection { /* model: ModelSpec, stream_fn: Arc<dyn StreamFn> */ }

impl ModelConnection {
    pub fn new(model: ModelSpec, stream_fn: Arc<dyn StreamFn>) -> Self;
    pub const fn model_spec(&self) -> &ModelSpec;
    pub fn stream_fn(&self) -> Arc<dyn StreamFn>;
}

/// Primary model connection plus deduplicated extras.
pub struct ModelConnections {
    /* primary_model, primary_stream_fn, extra_models */
}

impl ModelConnections {
    /// Construct with deduplication: extras matching the primary or each other are dropped.
    pub fn new(primary: ModelConnection, extras: Vec<ModelConnection>) -> Self;
    pub const fn primary_model(&self) -> &ModelSpec;
    pub fn primary_stream_fn(&self) -> Arc<dyn StreamFn>;
    pub fn extra_models(&self) -> &[(ModelSpec, Arc<dyn StreamFn>)];
    pub fn into_parts(self) -> (ModelSpec, Arc<dyn StreamFn>, Vec<(ModelSpec, Arc<dyn StreamFn>)>);
}
```

## `src/fallback.rs` — Fallback Configuration

```rust
/// Ordered sequence of fallback models for automatic failover.
///
/// The agent tries each model in order, applying the configured
/// RetryStrategy independently per model. When all are exhausted
/// the error propagates normally.
#[derive(Clone)]
pub struct ModelFallback { /* models: Vec<(ModelSpec, Arc<dyn StreamFn>)> */ }

impl ModelFallback {
    pub fn new(models: Vec<(ModelSpec, Arc<dyn StreamFn>)>) -> Self;
    pub fn models(&self) -> &[(ModelSpec, Arc<dyn StreamFn>)];
    pub fn is_empty(&self) -> bool;
    pub fn len(&self) -> usize;
}

impl Debug for ModelFallback { /* displays "provider:model_id" for each entry */ }
```

## Re-exports from `lib.rs`

```rust
pub use fallback::ModelFallback;
pub use model_catalog::{
    ApiVersion, AuthMode, CatalogPreset, ModelCatalog, PresetCapability, PresetCatalog,
    PresetStatus, ProviderCatalog, ProviderKind, model_catalog,
};
pub use model_presets::{ModelConnection, ModelConnections};
```
