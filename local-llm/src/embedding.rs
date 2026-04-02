//! Local embedding model for text vectorization.
//!
//! `EmbeddingModel` is a thin typed wrapper over the shared lazy-loader,
//! providing the embedding-specific build logic (via
//! `mistralrs::EmbeddingModelBuilder`) as a `LoaderBackend` implementation.

use std::future::Future;
use std::pin::Pin;

use tracing::{debug, error};

use crate::error::LocalModelError;
use crate::loader::{LazyLoader, LoaderBackend, LoaderState};
use crate::preset::ModelPreset;
use crate::progress::ProgressCallbackFn;

// ─── EmbeddingConfig ────────────────────────────────────────────────────────

/// Configuration for a local embedding model.
#[derive(Debug, Clone)]
pub struct EmbeddingConfig {
    /// `HuggingFace` repository ID for the embedding model.
    pub repo_id: String,

    /// Model filename (GGUF or safetensors).
    pub filename: String,

    /// Output embedding dimensions (default 768, configurable 128–768).
    pub dimensions: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            repo_id: std::env::var("LOCAL_EMBED_REPO")
                .unwrap_or_else(|_| "google/gemma-embedding-300m".to_string()),
            filename: std::env::var("LOCAL_EMBED_FILE").unwrap_or_else(|_| String::new()),
            dimensions: std::env::var("LOCAL_EMBED_DIMS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(768),
        }
    }
}

// ─── EmbeddingBackend ──────────────────────────────────────────────────────────

/// [`LoaderBackend`] for embedding models: downloads and builds via
/// `EmbeddingModelBuilder` (which handles its own download internally).
pub(crate) struct EmbeddingBackend;

impl LoaderBackend for EmbeddingBackend {
    type Config = EmbeddingConfig;
    /// Embedding builder handles its own download, so no intermediate artifact.
    type Artifact = ();

    fn download(
        config: &EmbeddingConfig,
        _progress_cb: Option<ProgressCallbackFn>,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Artifact, LocalModelError>> + Send + '_>> {
        Box::pin(async move {
            tracing::info!(repo = %config.repo_id, "downloading embedding model");
            // EmbeddingModelBuilder handles its own download — nothing to do here.
            Ok(())
        })
    }

    fn build(
        config: &EmbeddingConfig,
        _artifact: Self::Artifact,
        _progress_cb: Option<ProgressCallbackFn>,
    ) -> Pin<Box<dyn Future<Output = Result<mistralrs::Model, LocalModelError>> + Send + '_>> {
        Box::pin(async move {
            mistralrs::EmbeddingModelBuilder::new(config.repo_id.clone())
                .build()
                .await
                .map_err(|e| {
                    let msg = format!("embedding model loading failed: {e}");
                    error!(%msg);
                    LocalModelError::loading_message(msg)
                })
        })
    }

    fn label() -> &'static str {
        "embedding model"
    }
}

// ─── EmbeddingModel ─────────────────────────────────────────────────────────

/// A lazily-loaded local embedding model for text vectorization.
///
/// Wraps a shared lazy-loader for cheap cloning — multiple tasks can
/// share the same loaded model concurrently.
#[derive(Clone)]
pub struct EmbeddingModel {
    loader: LazyLoader<EmbeddingBackend>,
}

impl std::fmt::Debug for EmbeddingModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbeddingModel")
            .field("config", &self.loader.config())
            .finish_non_exhaustive()
    }
}

impl EmbeddingModel {
    /// Create a new `EmbeddingModel` in the `Unloaded` state.
    #[must_use]
    pub fn new(config: EmbeddingConfig) -> Self {
        Self {
            loader: LazyLoader::new(config),
        }
    }

    /// Create an `EmbeddingModel` from a [`ModelPreset`].
    #[must_use]
    pub fn from_preset(preset: ModelPreset) -> Self {
        Self::new(preset.embedding_config())
    }

    /// Attaches a progress callback for model download/load reporting.
    ///
    /// # Errors
    ///
    /// Returns `Err` if this instance has already been cloned (the internal `Arc` is
    /// shared). **Must be called before cloning the model**.
    pub fn with_progress(mut self, cb: ProgressCallbackFn) -> Result<Self, LocalModelError> {
        self.loader = self.loader.with_progress(cb)?;
        Ok(self)
    }

    /// Returns `true` if the model is loaded and ready.
    pub async fn is_ready(&self) -> bool {
        self.loader.is_ready().await
    }

    /// Idempotent: download → load → ready.
    pub async fn ensure_ready(&self) -> Result<(), LocalModelError> {
        self.loader.ensure_ready().await
    }

    /// Embed a single text, returning a fixed-dimensional vector.
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>, LocalModelError> {
        self.ensure_ready().await?;

        let state = self.loader.runner().await?;
        let LoaderState::Ready { runner } = &*state else {
            return Err(LocalModelError::NotReady);
        };

        let result = runner
            .generate_embedding(text)
            .await
            .map_err(|e| LocalModelError::embedding(format!("embedding failed: {e}")));
        drop(state);
        result
    }

    /// Embed a batch of texts, returning one vector per input.
    pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, LocalModelError> {
        self.ensure_ready().await?;

        let state = self.loader.runner().await?;
        let LoaderState::Ready { runner } = &*state else {
            return Err(LocalModelError::NotReady);
        };

        debug!(count = texts.len(), "sending embedding request");

        let mut request = mistralrs::EmbeddingRequestBuilder::new();
        for text in texts {
            request = request.add_prompt(*text);
        }

        let embeddings = runner
            .generate_embeddings(request)
            .await
            .map_err(|e| LocalModelError::embedding(format!("embedding failed: {e}")))?;

        drop(state);
        Ok(embeddings)
    }

    /// Drop the loaded model, returning to `Unloaded` state.
    pub async fn unload(&self) {
        self.loader.unload().await;
    }

    /// Access the configuration.
    pub fn config(&self) -> &EmbeddingConfig {
        self.loader.config()
    }
}

// ─── Compile-time assertions ────────────────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<EmbeddingModel>();
    assert_send_sync::<EmbeddingConfig>();
};

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn embedding_model_debug() {
        let model = EmbeddingModel::new(EmbeddingConfig::default());
        let debug = format!("{model:?}");
        assert!(debug.contains("EmbeddingModel"));
    }

    #[tokio::test]
    async fn new_model_is_not_ready() {
        let model = EmbeddingModel::new(EmbeddingConfig::default());
        assert!(!model.is_ready().await);
    }

    #[test]
    fn from_preset_creates_embedding_model() {
        let model = EmbeddingModel::from_preset(ModelPreset::EmbeddingGemma300M);
        let config = model.config();
        assert!(config.repo_id.contains("gemma"));
        assert_eq!(config.dimensions, 768);
    }

    #[test]
    fn with_progress_before_clone_succeeds() {
        let model = EmbeddingModel::new(EmbeddingConfig::default());
        let cb: ProgressCallbackFn = Arc::new(|_| {});
        let result = model.with_progress(cb);
        assert!(result.is_ok());
    }

    #[test]
    fn with_progress_after_clone_fails() {
        let model = EmbeddingModel::new(EmbeddingConfig::default());
        let _clone = model.clone();
        let cb: ProgressCallbackFn = Arc::new(|_| {});
        let result = model.with_progress(cb);
        assert!(result.is_err());
    }

    #[test]
    fn embedding_config_default() {
        let config = EmbeddingConfig::default();
        assert_eq!(config.dimensions, 768);
        assert!(config.repo_id.contains("gemma"));
    }
}
