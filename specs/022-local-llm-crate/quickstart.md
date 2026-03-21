# Quickstart: Local LLM Crate

**Branch**: `022-local-llm-crate` | **Date**: 2026-03-20

## Add the dependency

```toml
# In your crate's Cargo.toml
[dependencies]
swink-agent-local-llm = { path = "../local-llm" }
```

## Run local inference with a preset

```rust
use swink_agent_local_llm::{LocalModel, LocalStreamFn, ModelPreset};
use std::sync::Arc;

// Create a model from the default preset (SmolLM3-3B, Q4_K_M)
let model = LocalModel::from_preset(ModelPreset::SmolLM3_3B);

// Download and load the model (lazy — only downloads on first call)
model.ensure_ready().await?;

// Create a streaming function for the agent loop
let model = Arc::new(model);
let stream_fn = LocalStreamFn::new(Arc::clone(&model));

// Use it with the agent just like a cloud provider
let agent = Agent::new(stream_fn);
```

## Track download and loading progress

```rust
use swink_agent_local_llm::{LocalModel, ModelPreset, ProgressEvent};
use std::sync::Arc;

let model = LocalModel::from_preset(ModelPreset::SmolLM3_3B)
    .with_progress(Arc::new(|event| match event {
        ProgressEvent::DownloadProgress { bytes_downloaded, total_bytes } => {
            if let Some(total) = total_bytes {
                let pct = (bytes_downloaded as f64 / total as f64) * 100.0;
                println!("Downloading: {pct:.1}%");
            }
        }
        ProgressEvent::DownloadComplete => println!("Download complete"),
        ProgressEvent::LoadingProgress { message } => println!("Loading: {message}"),
        ProgressEvent::LoadingComplete => println!("Model ready"),
    }))?;

model.ensure_ready().await?;
```

## Compute text embeddings

```rust
use swink_agent_local_llm::{EmbeddingModel, ModelPreset};

// Create and load the embedding model
let embedder = EmbeddingModel::from_preset(ModelPreset::EmbeddingGemma300M);
embedder.ensure_ready().await?;

// Embed a single text
let vector = embedder.embed("How do I fix this error?").await?;
println!("Embedding dimension: {}", vector.len());

// Embed a batch of texts
let vectors = embedder.embed_batch(&[
    "How do I fix this error?",
    "What causes this bug?",
    "Make me a sandwich",
]).await?;

// Compare similarity (cosine similarity)
let similarity = cosine_similarity(&vectors[0], &vectors[1]);
println!("Similar pair: {similarity:.4}");  // Higher score

let dissimilarity = cosine_similarity(&vectors[0], &vectors[2]);
println!("Dissimilar pair: {dissimilarity:.4}");  // Lower score
```

## Use a custom model configuration

```rust
use swink_agent_local_llm::{LocalModel, ModelConfig};

let config = ModelConfig {
    repo_id: "my-org/my-model-GGUF".into(),
    filename: "my-model-q4_k_m.gguf".into(),
    context_length: 4096,
    chat_template: None,
};

let model = LocalModel::new(config);
model.ensure_ready().await?;
```

## Override context length via environment

```bash
# Override the default 8192 context window
export LOCAL_CONTEXT_LENGTH=4096
```

## Build and test

```bash
cargo build -p swink-agent-local-llm
cargo test -p swink-agent-local-llm

# Live tests (requires model download, ~2.1 GB)
cargo test -p swink-agent-local-llm --test local_live -- --ignored
cargo test -p swink-agent-local-llm --test embedding_live -- --ignored
```
