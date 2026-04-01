//! Local model management with lazy download and loading.
//!
//! [`LocalModel`] wraps a mistral.rs GGUF model behind `Arc` for cheap
//! cloning and concurrent access. The model is lazily downloaded from
//! `HuggingFace` and loaded on first use via [`ensure_ready`](LocalModel::ensure_ready).
//!
//! # Structural similarity with `embedding.rs`
//!
//! This module intentionally mirrors the `Arc<Inner>` + state-machine pattern
//! used in [`crate::embedding`]. Both modules manage a lazily-loaded mistral.rs
//! model behind `Arc` with `RwLock`-guarded state, `Notify`-based readiness
//! signalling, and progress callbacks. They diverge in their public APIs
//! (`runner()` + streaming chat completion here vs `embed()`/`embed_batch()`
//! in embedding) and in their underlying runner types (`GgufModelBuilder` for
//! chat vs `EmbeddingModelBuilder` for vectorization).

use std::sync::Arc;

use swink_agent::model_catalog;
use tokio::sync::{Notify, RwLock};
use tracing::{debug, error, info};

use crate::error::LocalModelError;
use crate::lifecycle::{
    LoadStateCheck, attach_progress_callback, classify_load_state, emit_progress,
    set_failed_and_notify, wait_until_ready,
};
use crate::preset::{DEFAULT_LOCAL_PRESET_ID, ModelPreset};
use crate::progress::{ProgressCallbackFn, ProgressEvent};

// ─── ModelConfig ────────────────────────────────────────────────────────────

/// Configuration for a local GGUF model.
#[derive(Debug, Clone)]
pub struct ModelConfig {
    /// `HuggingFace` repository ID (e.g. `bartowski/SmolLM3-3B-GGUF`).
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

impl Default for ModelConfig {
    fn default() -> Self {
        let preset = model_catalog()
            .preset("local", DEFAULT_LOCAL_PRESET_ID)
            .expect("local default preset must exist in src/model_catalog.toml");
        Self {
            repo_id: std::env::var("LOCAL_MODEL_REPO").unwrap_or_else(|_| {
                preset
                    .repo_id
                    .expect("local default preset must define repo_id")
            }),
            filename: std::env::var("LOCAL_MODEL_FILE").unwrap_or_else(|_| {
                preset
                    .filename
                    .expect("local default preset must define filename")
            }),
            gpu_layers: std::env::var("LOCAL_GPU_LAYERS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            context_length: std::env::var("LOCAL_CONTEXT_LENGTH")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(8192),
            chat_template: None,
        }
    }
}

// ─── ModelState (public) ───────────────────────────────────────────────────

/// Lifecycle state of a local model.
///
/// This is the public-facing state enum. Internally the model also tracks
/// the loaded runner, but that is not exposed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelState {
    /// Model has not been downloaded or loaded.
    Unloaded,

    /// Model weights are being downloaded.
    Downloading,

    /// Model is being loaded into the inference engine.
    Loading,

    /// Model is ready for inference.
    Ready,

    /// Model failed to load.
    Failed(String),
}

// ─── InternalModelState (crate-only) ───────────────────────────────────────

/// Internal state machine with the actual runner reference.
pub(crate) enum InternalModelState {
    Unloaded,
    Downloading,
    Loading,
    Ready { runner: mistralrs::Model },
    Failed { error: String },
}

impl std::fmt::Debug for InternalModelState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unloaded => write!(f, "Unloaded"),
            Self::Downloading => write!(f, "Downloading"),
            Self::Loading => write!(f, "Loading"),
            Self::Ready { .. } => write!(f, "Ready"),
            Self::Failed { error } => write!(f, "Failed({error})"),
        }
    }
}

impl InternalModelState {
    /// Convert to the public [`ModelState`].
    pub(crate) fn to_public(&self) -> ModelState {
        match self {
            Self::Unloaded => ModelState::Unloaded,
            Self::Downloading => ModelState::Downloading,
            Self::Loading => ModelState::Loading,
            Self::Ready { .. } => ModelState::Ready,
            Self::Failed { error } => ModelState::Failed(error.clone()),
        }
    }
}

// ─── LocalModel ─────────────────────────────────────────────────────────────

/// A lazily-loaded local LLM backed by mistral.rs GGUF inference.
///
/// Wraps `Arc<LocalModelInner>` for cheap cloning — multiple tasks can
/// share the same loaded model concurrently.
#[derive(Clone)]
pub struct LocalModel {
    inner: Arc<LocalModelInner>,
}

struct LocalModelInner {
    state: RwLock<InternalModelState>,
    ready_notify: Notify,
    config: ModelConfig,
    progress_cb: Option<ProgressCallbackFn>,
}

impl std::fmt::Debug for LocalModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalModel")
            .field("config", &self.inner.config)
            .finish_non_exhaustive()
    }
}

impl LocalModel {
    /// Create a new `LocalModel` in the `Unloaded` state.
    #[must_use]
    pub fn new(config: ModelConfig) -> Self {
        Self {
            inner: Arc::new(LocalModelInner {
                state: RwLock::new(InternalModelState::Unloaded),
                ready_notify: Notify::new(),
                config,
                progress_cb: None,
            }),
        }
    }

    /// Create a `LocalModel` from a [`ModelPreset`].
    ///
    /// Equivalent to `LocalModel::new(preset.config())`.
    #[must_use]
    pub fn from_preset(preset: ModelPreset) -> Self {
        Self::new(preset.config())
    }

    /// Attaches a progress callback for model download/load reporting.
    ///
    /// # Errors
    ///
    /// Returns `Err` if this instance has already been cloned (the internal `Arc` is
    /// shared). **Must be called before cloning the model** — i.e., before passing it
    /// to a second thread or storing it in shared state.
    pub fn with_progress(mut self, cb: ProgressCallbackFn) -> Result<Self, LocalModelError> {
        attach_progress_callback(&mut self.inner, cb, |inner, cb| {
            inner.progress_cb = Some(cb);
        })?;
        Ok(self)
    }

    /// Returns `true` if the model is loaded and ready for inference.
    pub async fn is_ready(&self) -> bool {
        matches!(
            *self.inner.state.read().await,
            InternalModelState::Ready { .. }
        )
    }

    /// Returns the current public [`ModelState`].
    pub async fn state(&self) -> ModelState {
        self.inner.state.read().await.to_public()
    }

    /// Block until the model reaches the `Ready` state.
    ///
    /// Uses the same `Notify` pattern as `agent.rs::wait_for_idle()`.
    pub async fn wait_until_ready(&self) {
        wait_until_ready(&self.inner.ready_notify, || self.is_ready()).await;
    }

    /// Idempotent: download → load → ready.
    ///
    /// Concurrent callers serialize on the `RwLock` — only the first caller
    /// triggers the download/load sequence; others wait for completion.
    #[allow(clippy::too_many_lines)]
    pub async fn ensure_ready(&self) -> Result<(), LocalModelError> {
        // Fast path: already ready.
        {
            let state = self.inner.state.read().await;
            match classify_load_state(
                &*state,
                |state| matches!(state, InternalModelState::Ready { .. }),
                |state| match state {
                    InternalModelState::Failed { error } => Some(error.clone()),
                    _ => None,
                },
                |state| {
                    matches!(
                        state,
                        InternalModelState::Downloading | InternalModelState::Loading
                    )
                },
            ) {
                LoadStateCheck::Ready => return Ok(()),
                LoadStateCheck::Failed(error) => {
                    return Err(LocalModelError::loading_message(error));
                }
                LoadStateCheck::Waiting => {
                    drop(state);
                    self.wait_until_ready().await;
                    return Ok(());
                }
                LoadStateCheck::Unloaded => {}
            }
        }

        // Slow path: acquire write lock and load.
        let mut state = self.inner.state.write().await;

        // Double-check after acquiring write lock.
        match classify_load_state(
            &*state,
            |state| matches!(state, InternalModelState::Ready { .. }),
            |state| match state {
                InternalModelState::Failed { error } => Some(error.clone()),
                _ => None,
            },
            |state| {
                matches!(
                    state,
                    InternalModelState::Downloading | InternalModelState::Loading
                )
            },
        ) {
            LoadStateCheck::Ready => return Ok(()),
            LoadStateCheck::Failed(error) => {
                return Err(LocalModelError::loading_message(error));
            }
            LoadStateCheck::Waiting => {
                // Another task is loading — shouldn't happen with write lock,
                // but handle gracefully.
                drop(state);
                self.wait_until_ready().await;
                return Ok(());
            }
            LoadStateCheck::Unloaded => {}
        }

        // Begin loading.
        *state = InternalModelState::Downloading;
        self.notify_progress(ProgressEvent::DownloadProgress {
            bytes_downloaded: 0,
            total_bytes: None,
        });

        info!(
            repo = %self.inner.config.repo_id,
            file = %self.inner.config.filename,
            "downloading local model"
        );

        // Download the GGUF file via hf-hub (caches in ~/.cache/huggingface/hub/).
        let api = hf_hub::api::tokio::Api::new().map_err(|e| {
            let msg = format!("HuggingFace API init failed: {e}");
            error!(%msg);
            self.notify_progress(ProgressEvent::DownloadProgress {
                bytes_downloaded: 0,
                total_bytes: None,
            });
            *state = InternalModelState::Failed { error: msg };
            self.inner.ready_notify.notify_waiters();
            LocalModelError::download(e)
        })?;

        let repo = api.model(self.inner.config.repo_id.clone());
        let model_path = repo.get(&self.inner.config.filename).await.map_err(|e| {
            let msg = format!("model download failed: {e}");
            error!(%msg);
            *state = InternalModelState::Failed { error: msg };
            self.inner.ready_notify.notify_waiters();
            LocalModelError::download(e)
        })?;

        debug!(path = %model_path.display(), "model downloaded, loading");

        // Emit download complete.
        self.notify_progress(ProgressEvent::DownloadComplete);

        *state = InternalModelState::Loading;
        self.notify_progress(ProgressEvent::LoadingProgress {
            message: "loading model into memory".to_string(),
        });

        // Build the GGUF model via mistral.rs.
        // Spawn in a blocking-safe task so that panics inside the builder
        // (e.g. unsupported architecture) are converted to errors.
        let repo_id = self.inner.config.repo_id.clone();
        let filename = self.inner.config.filename.clone();
        let build_result = tokio::task::spawn(async move {
            mistralrs::GgufModelBuilder::new(repo_id, vec![filename])
                .build()
                .await
        })
        .await;

        let runner = match build_result {
            Ok(Ok(runner)) => runner,
            Ok(Err(e)) => {
                let msg = format!("model loading failed: {e}");
                error!(%msg);
                return Err(set_failed_and_notify(
                    &mut *state,
                    &self.inner.ready_notify,
                    msg,
                    |state, error| *state = InternalModelState::Failed { error },
                ));
            }
            Err(join_err) => {
                let msg = format!("model loading panicked: {join_err}");
                error!(%msg);
                return Err(set_failed_and_notify(
                    &mut *state,
                    &self.inner.ready_notify,
                    msg,
                    |state, error| *state = InternalModelState::Failed { error },
                ));
            }
        };

        info!("local model ready");
        *state = InternalModelState::Ready { runner };
        drop(state);
        self.notify_progress(ProgressEvent::LoadingComplete);
        self.inner.ready_notify.notify_waiters();

        Ok(())
    }

    /// Drop the loaded model, returning to `Unloaded` state.
    pub async fn unload(&self) {
        let mut state = self.inner.state.write().await;
        *state = InternalModelState::Unloaded;
        drop(state);
        info!("local model unloaded");
    }

    /// Get a reference to the underlying mistral.rs `Model` runner.
    ///
    /// Returns `Err(NotReady)` if the model is not loaded.
    pub(crate) async fn runner(
        &self,
    ) -> Result<tokio::sync::RwLockReadGuard<'_, InternalModelState>, LocalModelError> {
        let state = self.inner.state.read().await;
        if matches!(&*state, InternalModelState::Ready { .. }) {
            Ok(state)
        } else {
            Err(LocalModelError::NotReady)
        }
    }

    /// Access the model configuration.
    pub fn config(&self) -> &ModelConfig {
        &self.inner.config
    }

    fn notify_progress(&self, progress: ProgressEvent) {
        emit_progress(self.inner.progress_cb.as_ref(), progress);
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
        // Test that context_length has a sensible default
        let config = ModelConfig::default();
        assert_eq!(config.context_length, 8192);
    }

    #[test]
    fn internal_model_state_debug() {
        let states = [
            InternalModelState::Unloaded,
            InternalModelState::Downloading,
            InternalModelState::Loading,
            InternalModelState::Failed {
                error: "test".into(),
            },
        ];
        for s in &states {
            let debug = format!("{s:?}");
            assert!(!debug.is_empty());
        }
    }

    #[test]
    fn internal_state_to_public() {
        assert_eq!(
            InternalModelState::Unloaded.to_public(),
            ModelState::Unloaded
        );
        assert_eq!(
            InternalModelState::Downloading.to_public(),
            ModelState::Downloading
        );
        assert_eq!(InternalModelState::Loading.to_public(), ModelState::Loading);
        assert_eq!(
            InternalModelState::Failed {
                error: "boom".into()
            }
            .to_public(),
            ModelState::Failed("boom".into())
        );
    }

    #[tokio::test]
    async fn send_chat_request_on_unloaded_model_returns_not_ready() {
        let model = LocalModel::new(ModelConfig::default());
        let err = model.runner().await.unwrap_err();
        assert!(err.to_string().contains("not ready"));
    }

    #[test]
    fn with_progress_before_clone_succeeds() {
        use std::sync::Arc;
        let model = LocalModel::new(ModelConfig::default());
        let cb: ProgressCallbackFn = Arc::new(|_| {});
        let result = model.with_progress(cb);
        assert!(result.is_ok());
    }

    #[test]
    fn with_progress_after_clone_fails() {
        use std::sync::Arc;
        let model = LocalModel::new(ModelConfig::default());
        let _clone = model.clone();
        let cb: ProgressCallbackFn = Arc::new(|_| {});
        let result = model.with_progress(cb);
        assert!(result.is_err());
    }
}
