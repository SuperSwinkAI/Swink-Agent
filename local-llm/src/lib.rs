#![forbid(unsafe_code)]
//! Local on-device LLM inference for swink-agent.
//!
//! Provides inference models and an embedding model, all lazily downloaded
//! from `HuggingFace` on first use via llama.cpp (GGUF format):
//!
//! - **SmolLM3-3B** (GGUF `Q4_K_M`, ~1.92 GB) — default text generation,
//!   tool use, and reasoning. Works on CPU-only hardware.
//! - **Gemma 4 E2B** (GGUF `Q4_K_M`, ~3.5 GB, requires `gemma4` feature) —
//!   opt-in 128K-context model with native thinking mode and tool calling.
//!   Use `LocalModel::from_preset(ModelPreset::Gemma4E2B)` to select it.
//! - **EmbeddingGemma-300M** (GGUF, <200 MB) — text vectorization/embeddings for
//!   semantic search and similarity.
//!
//! # GPU Requirements
//!
//! | Model | CPU | CUDA (NVIDIA) | Metal (Apple) |
//! |-------|-----|---------------|---------------|
//! | SmolLM3-3B | ✓ | ✓ `--features cuda` | ✓ `--features metal` |
//! | Gemma 4 E2B | ✓ | ✓ `--features cuda` | ✓ `--features metal` |
//! | EmbeddingGemma-300M | ✓ | ✓ | ✓ |
//!
//! All models are designed for `Arc<>` sharing so multiple in-process tasks
//! can use the loaded models concurrently.

mod convert;
pub mod embedding;
pub mod error;
pub(crate) mod loader;
pub mod model;
pub mod preset;
pub mod progress;
pub(crate) mod runner;
pub mod stream;

// Re-exports
pub use embedding::{EmbeddingConfig, EmbeddingModel};
pub use error::LocalModelError;
pub use model::{LocalModel, ModelConfig, ModelState};
pub use preset::{
    DEFAULT_LOCAL_PRESET_ID, LocalPresetError, ModelPreset, default_local_connection,
};
pub use progress::{ProgressCallbackFn, ProgressEvent};
pub use stream::LocalStreamFn;
