# Quickstart: Model Catalog, Presets & Fallback

**Feature**: 008-model-catalog-presets | **Date**: 2026-03-20

## Build & Test

```bash
# Build the workspace
cargo build --workspace

# Run all tests (includes catalog, connection, and fallback tests)
cargo test --workspace

# Lint (zero warnings policy)
cargo clippy --workspace -- -D warnings
```

## Usage Examples

### Browse the Model Catalog

```rust
use swink_agent::model_catalog;

let catalog = model_catalog();

// List all providers
for provider in &catalog.providers {
    println!("{} ({:?})", provider.display_name, provider.kind);
    for preset in &provider.presets {
        println!("  {} — {} ({:?})", preset.id, preset.display_name, preset.status);
    }
}

// Look up a specific provider
let anthropic = catalog.provider("anthropic").expect("anthropic provider exists");
assert_eq!(anthropic.credential_env_var.as_deref(), Some("ANTHROPIC_API_KEY"));
```

### Look Up a Preset with Provider Context

```rust
use swink_agent::model_catalog;

let preset = model_catalog()
    .preset("anthropic", "sonnet_46")
    .expect("preset exists");

// Flattened view includes both provider and preset metadata
assert_eq!(preset.provider_key, "anthropic");
assert_eq!(preset.model_id, "claude-sonnet-4-6");
assert_eq!(preset.credential_env_var.as_deref(), Some("ANTHROPIC_API_KEY"));
assert_eq!(preset.default_base_url.as_deref(), Some("https://api.anthropic.com"));
```

### Convert a Preset to ModelCapabilities

```rust
use swink_agent::model_catalog;

let preset = model_catalog().preset("openai", "gpt_4o").unwrap();
let caps = preset.model_capabilities();

assert!(caps.supports_tool_use);
assert!(caps.supports_vision);
assert!(!caps.supports_thinking);  // GPT-4o does not support extended thinking
assert_eq!(caps.max_context_window, Some(128_000));
```

### Create a ModelSpec from a Preset

```rust
use swink_agent::model_catalog;

let preset = model_catalog().preset("anthropic", "opus_46").unwrap();
let spec = preset.model_spec();

// ModelSpec carries provider, model_id, and capabilities
assert_eq!(spec.provider, "anthropic");
assert_eq!(spec.model_id, "claude-opus-4-6");
assert!(spec.capabilities().supports_thinking);
```

### Build a ModelConnection

```rust
use std::sync::Arc;
use swink_agent::{ModelConnection, ModelSpec};

// stream_fn comes from the adapters crate (provider-specific)
# fn make_stream_fn() -> Arc<dyn swink_agent::StreamFn> { todo!() }

let model = ModelSpec::new("anthropic", "claude-sonnet-4-6");
let conn = ModelConnection::new(model, make_stream_fn());

assert_eq!(conn.model_spec().provider, "anthropic");
```

### Configure Multiple Model Connections

```rust
use std::sync::Arc;
use swink_agent::{ModelConnection, ModelConnections, ModelSpec};
# fn make_stream_fn() -> Arc<dyn swink_agent::StreamFn> { todo!() }

let primary = ModelConnection::new(
    ModelSpec::new("anthropic", "claude-sonnet-4-6"),
    make_stream_fn(),
);

let extras = vec![
    ModelConnection::new(ModelSpec::new("openai", "gpt-4o"), make_stream_fn()),
    ModelConnection::new(ModelSpec::new("local", "SmolLM3-3B-Q4_K_M"), make_stream_fn()),
];

let connections = ModelConnections::new(primary, extras);

// Primary is always the first model
assert_eq!(connections.primary_model().provider, "anthropic");

// Extras are deduplicated
assert_eq!(connections.extra_models().len(), 2);

// Destructure for use
let (model, stream_fn, extras) = connections.into_parts();
```

### Configure Model Fallback

```rust
use std::sync::Arc;
use swink_agent::{ModelFallback, ModelSpec};
# fn make_stream_fn() -> Arc<dyn swink_agent::StreamFn> { todo!() }

// Fallback chain: try GPT-4o Mini first, then Haiku
let fallback = ModelFallback::new(vec![
    (ModelSpec::new("openai", "gpt-4o-mini"), make_stream_fn()),
    (ModelSpec::new("anthropic", "claude-haiku-4-5-20251001"), make_stream_fn()),
]);

assert_eq!(fallback.len(), 2);
assert!(!fallback.is_empty());

// Pass to AgentLoopConfig:
// config.model_fallback = Some(fallback);
```

## Adding a New Provider or Model

Edit `src/model_catalog.toml` — no code changes needed:

```toml
[[providers]]
key = "new_provider"
display_name = "New Provider"
kind = "remote"
auth_mode = "bearer"
credential_env_var = "NEW_PROVIDER_API_KEY"
base_url_env_var = "NEW_PROVIDER_BASE_URL"
default_base_url = "https://api.newprovider.com"

[[providers.presets]]
id = "new_model"
display_name = "New Provider Model"
group = "default"
model_id = "new-model-v1"
capabilities = ["text", "tools", "streaming"]
status = "ga"
context_window_tokens = 128000
max_output_tokens = 8192
```

Run `cargo test --workspace` to verify the updated catalog loads correctly.
