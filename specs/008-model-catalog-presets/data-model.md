# Data Model: Model Catalog, Presets & Fallback

**Feature**: 008-model-catalog-presets | **Date**: 2026-03-20

## Entities

### ModelCatalog (struct)

Top-level registry of providers and their presets. Loaded once from embedded TOML via `OnceLock`.

| Field | Type | Description |
|---|---|---|
| `providers` | `Vec<ProviderCatalog>` | All registered providers (default: empty) |

**Methods**:
- `provider(&self, provider_key: &str) -> Option<&ProviderCatalog>` — find provider by key
- `preset(&self, provider_key: &str, preset_id: &str) -> Option<CatalogPreset>` — find preset, returns flattened view with provider context

**Derives**: `Debug`, `Clone`, `PartialEq`, `Eq`, `Deserialize`

---

### ProviderCatalog (struct)

Metadata for an LLM provider — credentials, endpoints, auth configuration.

| Field | Type | Description |
|---|---|---|
| `key` | `String` | Unique provider identifier (e.g., `"anthropic"`, `"openai"`) |
| `display_name` | `String` | Human-readable name (e.g., `"Anthropic"`) |
| `kind` | `ProviderKind` | Remote or local provider |
| `auth_mode` | `Option<AuthMode>` | Authentication method (bearer, API key header, AWS SigV4) |
| `credential_env_var` | `Option<String>` | Environment variable name for credentials |
| `base_url_env_var` | `Option<String>` | Environment variable name for custom base URL |
| `default_base_url` | `Option<String>` | Default API base URL |
| `requires_base_url` | `bool` | Whether a base URL must be provided (default: `false`) |
| `region_env_var` | `Option<String>` | Environment variable for region (AWS-specific) |
| `presets` | `Vec<PresetCatalog>` | Model presets for this provider |

**Methods**:
- `preset(&self, preset_id: &str) -> Option<&PresetCatalog>` — find preset by ID within this provider

**Derives**: `Debug`, `Clone`, `PartialEq`, `Eq`, `Deserialize`

---

### PresetCatalog (struct)

Metadata for a specific model preset within a provider.

| Field | Type | Description |
|---|---|---|
| `id` | `String` | Unique preset identifier within the provider (e.g., `"sonnet_46"`) |
| `display_name` | `String` | Human-readable name (e.g., `"Anthropic Sonnet 4.6"`) |
| `group` | `Option<String>` | Model family grouping (e.g., `"sonnet"`, `"gpt4o"`) |
| `model_id` | `String` | Model identifier sent to the provider API |
| `api_version` | `Option<ApiVersion>` | API version override (e.g., `v1beta` for Google) |
| `capabilities` | `Vec<PresetCapability>` | List of model capabilities (default: empty) |
| `status` | `Option<PresetStatus>` | Release status (GA, preview) |
| `context_window_tokens` | `Option<u64>` | Maximum context window size in tokens |
| `max_output_tokens` | `Option<u64>` | Maximum output tokens per response |
| `include_by_default` | `bool` | Whether to include in default model list (default: `false`) |
| `repo_id` | `Option<String>` | HuggingFace repository ID (local models) |
| `filename` | `Option<String>` | Model filename (local models) |

**Derives**: `Debug`, `Clone`, `PartialEq`, `Eq`, `Deserialize`

---

### CatalogPreset (struct)

Flattened, denormalized view combining provider and preset metadata. Not deserialized — constructed by `ModelCatalog::preset()`.

| Field | Type | Description |
|---|---|---|
| `provider_key` | `String` | Provider identifier |
| `provider_display_name` | `String` | Provider display name |
| `provider_kind` | `ProviderKind` | Remote or local |
| `preset_id` | `String` | Preset identifier |
| `display_name` | `String` | Preset display name |
| `group` | `Option<String>` | Model family grouping |
| `model_id` | `String` | Model identifier for API calls |
| `api_version` | `Option<ApiVersion>` | API version override |
| `capabilities` | `Vec<PresetCapability>` | Model capabilities |
| `status` | `Option<PresetStatus>` | Release status |
| `context_window_tokens` | `Option<u64>` | Context window size |
| `max_output_tokens` | `Option<u64>` | Max output tokens |
| `auth_mode` | `Option<AuthMode>` | Provider auth method |
| `credential_env_var` | `Option<String>` | Credential env var name |
| `base_url_env_var` | `Option<String>` | Base URL env var name |
| `default_base_url` | `Option<String>` | Default base URL |
| `requires_base_url` | `bool` | Whether base URL is required |
| `region_env_var` | `Option<String>` | Region env var name |
| `include_by_default` | `bool` | Default inclusion flag |
| `repo_id` | `Option<String>` | HuggingFace repo ID |
| `filename` | `Option<String>` | Model filename |

**Methods**:
- `model_capabilities(&self) -> ModelCapabilities` — convert capability list to `ModelCapabilities` struct
- `model_spec(&self) -> ModelSpec` — create a `ModelSpec` with capabilities pre-populated

**Derives**: `Debug`, `Clone`, `PartialEq`, `Eq`

---

### ProviderKind (enum)

| Variant | Serde Value | Description |
|---|---|---|
| `Remote` | `"remote"` | Cloud-hosted provider |
| `Local` | `"local"` | Locally-run model |

**Derives**: `Debug`, `Clone`, `PartialEq`, `Eq`, `Deserialize`. Serde: `rename_all = "snake_case"`.

---

### AuthMode (enum)

| Variant | Serde Value | Description |
|---|---|---|
| `Bearer` | `"bearer"` | Bearer token in Authorization header |
| `ApiKeyHeader` | `"api_key_header"` | API key in a custom header |
| `AwsSigv4` | `"aws_sigv4"` | AWS Signature Version 4 |

**Derives**: `Debug`, `Clone`, `PartialEq`, `Eq`, `Deserialize`. Serde: `rename_all = "snake_case"`.

---

### ApiVersion (enum)

| Variant | Serde Value | Description |
|---|---|---|
| `V1` | `"v1"` | Stable v1 API |
| `V1beta` | `"v1beta"` | Beta v1 API |

**Derives**: `Debug`, `Clone`, `PartialEq`, `Eq`, `Deserialize`. Serde: `rename_all = "snake_case"`.

---

### PresetCapability (enum)

| Variant | Serde Value | Description |
|---|---|---|
| `Text` | `"text"` | Text generation |
| `Tools` | `"tools"` | Tool/function calling |
| `Thinking` | `"thinking"` | Extended reasoning |
| `ImagesIn` | `"images_in"` | Vision/image input |
| `Streaming` | `"streaming"` | Streaming responses |
| `StructuredOutput` | `"structured_output"` | JSON mode / structured output |

**Derives**: `Debug`, `Clone`, `PartialEq`, `Eq`, `Deserialize`. Serde: `rename_all = "snake_case"`.

---

### PresetStatus (enum)

| Variant | Serde Value | Description |
|---|---|---|
| `Ga` | `"ga"` | Generally available |
| `Preview` | `"preview"` | Preview/beta release |

**Derives**: `Debug`, `Clone`, `PartialEq`, `Eq`, `Deserialize`. Serde: `rename_all = "snake_case"`.

---

### ModelConnection (struct)

A fully resolved model connection pairing a model specification with its streaming function.

| Field | Type | Description |
|---|---|---|
| `model` | `ModelSpec` | Model specification (provider + model ID + capabilities) |
| `stream_fn` | `Arc<dyn StreamFn>` | Provider-specific streaming implementation |

**Methods**:
- `new(model: ModelSpec, stream_fn: Arc<dyn StreamFn>) -> Self`
- `model_spec(&self) -> &ModelSpec`
- `stream_fn(&self) -> Arc<dyn StreamFn>` — returns cloned `Arc`

**Derives**: `Clone`

---

### ModelConnections (struct)

A primary model connection plus deduplicated extra connections. Deduplication occurs at construction time based on `ModelSpec` equality.

| Field | Type | Description |
|---|---|---|
| `primary_model` | `ModelSpec` | Primary model specification |
| `primary_stream_fn` | `Arc<dyn StreamFn>` | Primary streaming function |
| `extra_models` | `Vec<(ModelSpec, Arc<dyn StreamFn>)>` | Additional model connections (deduplicated) |

**Methods**:
- `new(primary: ModelConnection, extras: Vec<ModelConnection>) -> Self` — deduplicates extras against primary and each other
- `primary_model(&self) -> &ModelSpec`
- `primary_stream_fn(&self) -> Arc<dyn StreamFn>`
- `extra_models(&self) -> &[(ModelSpec, Arc<dyn StreamFn>)]`
- `into_parts(self) -> (ModelSpec, Arc<dyn StreamFn>, Vec<(ModelSpec, Arc<dyn StreamFn>)>)` — destructure into components

---

### ModelFallback (struct)

An ordered sequence of fallback models. The agent loop tries each in order, applying the configured `RetryStrategy` independently per model. When all are exhausted, the last error propagates.

| Field | Type | Description |
|---|---|---|
| `models` | `Vec<(ModelSpec, Arc<dyn StreamFn>)>` | Ordered fallback chain |

**Methods**:
- `new(models: Vec<(ModelSpec, Arc<dyn StreamFn>)>) -> Self`
- `models(&self) -> &[(ModelSpec, Arc<dyn StreamFn>)]`
- `is_empty(&self) -> bool`
- `len(&self) -> usize`

**Derives**: `Clone`. Custom `Debug` impl (displays model specs without stream functions).

---

### model_catalog() (free function)

```text
pub fn model_catalog() -> &'static ModelCatalog
```

Returns a reference to the singleton `ModelCatalog` loaded from the embedded TOML file. Uses `OnceLock` for thread-safe lazy initialization. Panics with a descriptive message if the TOML is malformed.

---

## Relationships

```text
model_catalog() -> &'static ModelCatalog
                            │
                            ├── providers: Vec<ProviderCatalog>
                            │       │
                            │       ├── ProviderKind (Remote | Local)
                            │       ├── AuthMode (Bearer | ApiKeyHeader | AwsSigv4)
                            │       └── presets: Vec<PresetCatalog>
                            │               │
                            │               ├── ApiVersion (V1 | V1beta)
                            │               ├── PresetCapability (Text | Tools | ...)
                            │               └── PresetStatus (Ga | Preview)
                            │
                            └── preset() -> CatalogPreset (flattened provider + preset)
                                    │
                                    ├── model_capabilities() -> ModelCapabilities
                                    └── model_spec() -> ModelSpec

ModelConnection (ModelSpec + Arc<dyn StreamFn>)
    └── used by ModelConnections (primary + deduplicated extras)

ModelFallback (Vec<(ModelSpec, Arc<dyn StreamFn>)>)
    └── consumed by agent loop for automatic failover
```
