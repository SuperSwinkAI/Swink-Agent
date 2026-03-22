#![forbid(unsafe_code)]
//! Local on-device LLM inference for swink-agent.
//!
//! Provides two models, both lazily downloaded from `HuggingFace` on first use:
//!
//! - **SmolLM3-3B** (GGUF `Q4_K_M`, ~1.92 GB) — text generation, tool use,
//!   reasoning. Default fallback when no cloud API credentials are configured.
//! - **EmbeddingGemma-300M** (<200 MB) — text vectorization/embeddings for
//!   semantic search and similarity.
//!
//! Both models are designed for `Arc<>` sharing so multiple in-process tasks
//! can use the loaded models concurrently.

mod convert;
pub mod embedding;
pub mod error;
pub mod model;
pub mod preset;
pub mod progress;
pub mod stream;

// Re-exports
pub use embedding::{EmbeddingConfig, EmbeddingModel};
pub use error::LocalModelError;
pub use model::{LocalModel, ModelConfig, ModelState};
pub use preset::{DEFAULT_LOCAL_PRESET_ID, LocalPresetError, ModelPreset, default_local_connection};
pub use progress::{ProgressCallbackFn, ProgressEvent};
pub use stream::LocalStreamFn;
