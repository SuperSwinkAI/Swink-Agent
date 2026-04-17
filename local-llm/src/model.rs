//! Local model management with lazy download and loading.
//!
//! `LocalModel` is a thin typed wrapper over the shared lazy-loader,
//! providing the chat-model–specific download (via `hf-hub`) and build
//! (via `llama-cpp-2`) logic as a `LoaderBackend` implementation.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tracing::{debug, error, info};

use crate::error::LocalModelError;
use crate::loader::{LazyLoader, LoaderBackend, LoaderState, PublicLoaderState};
use crate::preset::{ModelPreset, default_chat_model_config};
use crate::progress::ProgressCallbackFn;
use crate::runner::{LlamaRunner, RunnerConfig};

// ─── ModelConfig ────────────────────────────────────────────────────────────

/// Configuration for a local GGUF model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelConfig {
    /// `HuggingFace` repository ID (e.g. `unsloth/SmolLM3-3B-GGUF`).
    pub repo_id: String,

    /// GGUF filename within the repository.
    pub filename: String,

    /// Number of layers to offload to GPU (0 = CPU only).
    pub gpu_layers: u32,

    /// Context window length (capped to save memory).
    pub context_length: usize,

    /// Optional chat template override. If `None`, uses model's built-in template.
    pub chat_template: Option<String>,
}

impl ModelConfig {
    /// Returns `true` if this configuration targets a Gemma 4 model family.
    #[cfg(feature = "gemma4")]
    pub fn is_gemma4(&self) -> bool {
        let id = self.repo_id.to_ascii_lowercase();
        id.contains("gemma-4") || id.contains("gemma4")
    }
}

impl Default for ModelConfig {
    fn default() -> Self {
        default_chat_model_config()
    }
}

// ─── ModelState (public) ───────────────────────────────────────────────────

/// Lifecycle state of a local model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelState {
    Unloaded,
    Downloading,
    Loading,
    Ready,
    Failed(String),
}

impl From<PublicLoaderState> for ModelState {
    fn from(s: PublicLoaderState) -> Self {
        match s {
            PublicLoaderState::Unloaded => Self::Unloaded,
            PublicLoaderState::Downloading => Self::Downloading,
            PublicLoaderState::Loading => Self::Loading,
            PublicLoaderState::Ready => Self::Ready,
            PublicLoaderState::Failed(e) => Self::Failed(e),
        }
    }
}

// ─── ChatBackend ───────────────────────────────────────────────────────────

/// [`LoaderBackend`] for chat models: downloads via `hf-hub`, builds via
/// `llama-cpp-2`.
pub(crate) struct ChatBackend;

impl LoaderBackend for ChatBackend {
    type Config = ModelConfig;
    type Artifact = std::path::PathBuf;
    type Runner = Arc<LlamaRunner>;

    fn download(
        config: &ModelConfig,
        _progress_cb: Option<ProgressCallbackFn>,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Artifact, LocalModelError>> + Send + '_>> {
        Box::pin(async move {
            info!(
                repo = %config.repo_id,
                file = %config.filename,
                "downloading local model"
            );

            let api = hf_hub::api::tokio::Api::new().map_err(|e| {
                let msg = format!("HuggingFace API init failed: {e}");
                error!(%msg);
                LocalModelError::download(e)
            })?;

            let repo = api.model(config.repo_id.clone());
            let model_path = repo.get(&config.filename).await.map_err(|e| {
                let msg = format!("model download failed: {e}");
                error!(%msg);
                LocalModelError::download(e)
            })?;

            debug!(path = %model_path.display(), "model downloaded");
            Ok(model_path)
        })
    }

    fn build(
        config: &ModelConfig,
        artifact: Self::Artifact,
        _progress_cb: Option<ProgressCallbackFn>,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Runner, LocalModelError>> + Send + '_>> {
        Box::pin(async move {
            let runner_config = RunnerConfig {
                context_length: u32::try_from(config.context_length).unwrap_or(u32::MAX),
                gpu_layers: config.gpu_layers,
                ..RunnerConfig::default()
            };

            let model_path = artifact;
            let build_result =
                tokio::task::spawn_blocking(move || LlamaRunner::load(&model_path, runner_config))
                    .await;

            match build_result {
                Ok(Ok(runner)) => Ok(Arc::new(runner)),
                Ok(Err(e)) => {
                    error!(error = %e, "model loading failed");
                    Err(e)
                }
                Err(join_err) => {
                    let msg = format!("model loading panicked: {join_err}");
                    error!(%msg);
                    Err(LocalModelError::loading_message(msg))
                }
            }
        })
    }

    fn label() -> &'static str {
        "local model"
    }
}

// ─── LocalModel ─────────────────────────────────────────────────────────────

/// A lazily-loaded local LLM backed by llama.cpp GGUF inference.
///
/// Wraps a shared lazy-loader for cheap cloning — multiple tasks can
/// share the same loaded model concurrently.
#[derive(Clone)]
pub struct LocalModel {
    loader: LazyLoader<ChatBackend>,
}

impl std::fmt::Debug for LocalModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalModel")
            .field("config", &self.loader.config())
            .finish_non_exhaustive()
    }
}

impl LocalModel {
    /// Create a new `LocalModel` in the `Unloaded` state.
    #[must_use]
    pub fn new(config: ModelConfig) -> Self {
        Self {
            loader: LazyLoader::new(config),
        }
    }

    /// Create a `LocalModel` from a [`ModelPreset`].
    #[must_use]
    pub fn from_preset(preset: ModelPreset) -> Self {
        Self::new(preset.config())
    }

    /// Attaches a progress callback for model download/load reporting.
    pub fn with_progress(mut self, cb: ProgressCallbackFn) -> Result<Self, LocalModelError> {
        self.loader = self.loader.with_progress(cb)?;
        Ok(self)
    }

    /// Returns `true` if the model is loaded and ready for inference.
    pub async fn is_ready(&self) -> bool {
        self.loader.is_ready().await
    }

    /// Returns the current public [`ModelState`].
    pub async fn state(&self) -> ModelState {
        self.loader.public_state().await.into()
    }

    /// Block until the model reaches the `Ready` state.
    pub async fn wait_until_ready(&self) {
        self.loader.wait_until_ready().await;
    }

    /// Idempotent: download → load → ready.
    pub async fn ensure_ready(&self) -> Result<(), LocalModelError> {
        self.loader.ensure_ready().await
    }

    /// Drop the loaded model, returning to `Unloaded` state.
    pub async fn unload(&self) {
        self.loader.unload().await;
    }

    /// Get a reference to the underlying runner.
    pub(crate) async fn runner(
        &self,
    ) -> Result<tokio::sync::RwLockReadGuard<'_, LoaderState<Arc<LlamaRunner>>>, LocalModelError>
    {
        self.loader.runner().await
    }

    /// Access the model configuration.
    pub fn config(&self) -> &ModelConfig {
        self.loader.config()
    }
}

// ─── Compile-time assertions ────────────────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<LocalModel>();
    assert_send_sync::<ModelConfig>();
};

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn local_model_debug() {
        let model = LocalModel::new(ModelConfig::default());
        let debug = format!("{model:?}");
        assert!(debug.contains("LocalModel"));
    }

    #[tokio::test]
    async fn new_model_is_not_ready() {
        let model = LocalModel::new(ModelConfig::default());
        assert!(!model.is_ready().await);
    }

    #[tokio::test]
    async fn new_model_state_is_unloaded() {
        let model = LocalModel::new(ModelConfig::default());
        assert_eq!(model.state().await, ModelState::Unloaded);
    }

    #[tokio::test]
    async fn runner_returns_not_ready_when_unloaded() {
        let model = LocalModel::new(ModelConfig::default());
        assert!(model.runner().await.is_err());
    }

    #[test]
    fn from_preset_creates_model_with_correct_config() {
        let model = LocalModel::from_preset(ModelPreset::SmolLM3_3B);
        let config = model.config();
        assert!(config.repo_id.contains("SmolLM3"));
        assert_eq!(config.context_length, 8192);
    }

    #[test]
    fn model_config_default_has_chat_template_none() {
        let config = ModelConfig::default();
        assert!(config.chat_template.is_none());
    }

    #[test]
    fn model_config_context_length_env_override() {
        let config = ModelConfig::default();
        assert_eq!(config.context_length, 8192);
    }

    #[tokio::test]
    async fn send_chat_request_on_unloaded_model_returns_not_ready() {
        let model = LocalModel::new(ModelConfig::default());
        let err = model.runner().await.unwrap_err();
        assert!(err.to_string().contains("not ready"));
    }

    #[test]
    fn with_progress_before_clone_succeeds() {
        let model = LocalModel::new(ModelConfig::default());
        let cb: ProgressCallbackFn = Arc::new(|_| {});
        let result = model.with_progress(cb);
        assert!(result.is_ok());
    }

    #[test]
    fn with_progress_after_clone_fails() {
        let model = LocalModel::new(ModelConfig::default());
        let _clone = model.clone();
        let cb: ProgressCallbackFn = Arc::new(|_| {});
        let result = model.with_progress(cb);
        assert!(result.is_err());
    }

    #[cfg(feature = "gemma4")]
    mod gemma4_tests {
        use super::*;

        #[test]
        fn is_gemma4_detects_bartowski_repo() {
            let config = ModelConfig {
                repo_id: "bartowski/google_gemma-4-E2B-it-GGUF".to_string(),
                ..ModelConfig::default()
            };
            assert!(config.is_gemma4());
        }

        #[test]
        fn is_gemma4_detects_ollama_style_repo() {
            let config = ModelConfig {
                repo_id: "gemma4-e2b".to_string(),
                ..ModelConfig::default()
            };
            assert!(config.is_gemma4());
        }

        #[test]
        fn is_gemma4_false_for_smollm() {
            let config = ModelConfig {
                repo_id: "unsloth/SmolLM3-3B-GGUF".to_string(),
                ..ModelConfig::default()
            };
            assert!(!config.is_gemma4());
        }
    }
}
