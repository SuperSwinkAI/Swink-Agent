//! Local embedding model for text vectorization.
//!
//! `EmbeddingModel` is a thin typed wrapper over the shared lazy-loader,
//! providing the embedding-specific build logic (via `llama-cpp-2`) as a
//! `LoaderBackend` implementation.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tracing::{debug, error, info};

use crate::error::LocalModelError;
use crate::loader::{LazyLoader, LoaderBackend, LoaderState};
use crate::preset::{ModelPreset, default_embedding_config};
use crate::progress::{ProgressCallbackFn, resolve_model_path};
use crate::runner::{LlamaRunner, RunnerConfig};

// ─── EmbeddingConfig ────────────────────────────────────────────────────────

/// Configuration for a local embedding model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingConfig {
    /// `HuggingFace` repository ID for the embedding model.
    pub repo_id: String,

    /// Model filename (GGUF).
    pub filename: String,

    /// Output embedding dimensions (default 768, configurable 128–768).
    pub dimensions: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        default_embedding_config()
    }
}

// ─── EmbeddingBackend ──────────────────────────────────────────────────────

pub(crate) struct EmbeddingBackend;

impl LoaderBackend for EmbeddingBackend {
    type Config = EmbeddingConfig;
    type Artifact = std::path::PathBuf;
    type Runner = Arc<LlamaRunner>;

    fn download(
        config: &EmbeddingConfig,
        progress_cb: Option<ProgressCallbackFn>,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Artifact, LocalModelError>> + Send + '_>> {
        Box::pin(async move {
            info!(repo = %config.repo_id, "downloading embedding model");

            let model_path = resolve_model_path(&config.repo_id, &config.filename, progress_cb)
                .await
                .map_err(|e| {
                    error!(error = %e, "embedding model download failed");
                    LocalModelError::download(e)
                })?;

            debug!(path = %model_path.display(), "embedding model downloaded");
            Ok(model_path)
        })
    }

    fn build(
        _config: &EmbeddingConfig,
        artifact: Self::Artifact,
        progress_cb: Option<ProgressCallbackFn>,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Runner, LocalModelError>> + Send + '_>> {
        Box::pin(async move {
            let runner_config = RunnerConfig {
                context_length: 2048,
                gpu_layers: 0,
                ..RunnerConfig::default()
            };

            let model_path = artifact;
            let build_result = tokio::task::spawn_blocking(move || {
                LlamaRunner::load_with_progress(&model_path, runner_config, progress_cb.as_ref())
            })
            .await;

            match build_result {
                Ok(Ok(runner)) => Ok(Arc::new(runner)),
                Ok(Err(e)) => {
                    error!(error = %e, "embedding model loading failed");
                    Err(e)
                }
                Err(join_err) => {
                    let msg = format!("embedding model loading panicked: {join_err}");
                    error!(%msg);
                    Err(LocalModelError::loading_message(msg))
                }
            }
        })
    }

    fn label() -> &'static str {
        "embedding model"
    }
}

// ─── EmbeddingModel ─────────────────────────────────────────────────────────

/// A lazily-loaded local embedding model for text vectorization.
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
    #[must_use]
    pub fn new(config: EmbeddingConfig) -> Self {
        Self {
            loader: LazyLoader::new(config),
        }
    }

    #[must_use]
    pub fn from_preset(preset: ModelPreset) -> Self {
        Self::new(preset.embedding_config())
    }

    pub fn with_progress(mut self, cb: ProgressCallbackFn) -> Result<Self, LocalModelError> {
        self.loader = self.loader.with_progress(cb)?;
        Ok(self)
    }

    pub async fn is_ready(&self) -> bool {
        self.loader.is_ready().await
    }

    pub async fn ensure_ready(&self) -> Result<(), LocalModelError> {
        self.loader.ensure_ready().await
    }

    pub async fn embed(&self, text: &str) -> Result<Vec<f32>, LocalModelError> {
        self.ensure_ready().await?;

        let state = self.loader.runner().await?;
        let LoaderState::Ready { runner } = &*state else {
            return Err(LocalModelError::NotReady);
        };

        let runner = Arc::clone(runner);
        let owned_text = text.to_string();
        drop(state);

        tokio::task::spawn_blocking(move || runner.generate_embedding(&owned_text))
            .await
            .map_err(|e| LocalModelError::embedding(format!("embedding task panicked: {e}")))?
    }

    pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, LocalModelError> {
        self.ensure_ready().await?;

        let state = self.loader.runner().await?;
        let LoaderState::Ready { runner } = &*state else {
            return Err(LocalModelError::NotReady);
        };

        debug!(count = texts.len(), "sending embedding request");

        let runner = Arc::clone(runner);
        let owned_texts: Vec<String> = texts.iter().map(|t| (*t).to_string()).collect();
        drop(state);

        tokio::task::spawn_blocking(move || {
            let refs: Vec<&str> = owned_texts.iter().map(String::as_str).collect();
            runner.generate_embeddings_batch(&refs)
        })
        .await
        .map_err(|e| LocalModelError::embedding(format!("embedding task panicked: {e}")))?
    }

    pub async fn unload(&self) {
        self.loader.unload().await;
    }

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
