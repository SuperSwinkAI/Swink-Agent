//! Local model management with lazy download and loading.
//!
//! [`LocalModel`] wraps a mistral.rs GGUF model behind `Arc` for cheap
//! cloning and concurrent access. The model is lazily downloaded from
//! `HuggingFace` and loaded on first use via [`ensure_ready`](LocalModel::ensure_ready).

use std::sync::Arc;

use tokio::sync::{Notify, RwLock};
use tracing::{debug, error, info};

use crate::error::LocalModelError;
use crate::progress::{ModelProgress, ProgressCallbackFn};

// ─── ModelConfig ────────────────────────────────────────────────────────────

/// Configuration for a local GGUF model.
#[derive(Debug, Clone)]
pub struct ModelConfig {
    /// `HuggingFace` repository ID (e.g. `ggml-org/SmolLM3-3B-GGUF`).
    pub repo_id: String,

    /// GGUF filename within the repository.
    pub filename: String,

    /// Number of layers to offload to GPU (0 = CPU only).
    pub gpu_layers: u32,

    /// Context window length (capped to save memory).
    pub context_length: usize,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            repo_id: std::env::var("LOCAL_MODEL_REPO")
                .unwrap_or_else(|_| "ggml-org/SmolLM3-3B-GGUF".to_string()),
            filename: std::env::var("LOCAL_MODEL_FILE")
                .unwrap_or_else(|_| "SmolLM3-3B-Q4_K_M.gguf".to_string()),
            gpu_layers: std::env::var("LOCAL_GPU_LAYERS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            context_length: std::env::var("LOCAL_CONTEXT_LENGTH")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(8192),
        }
    }
}

// ─── ModelState ─────────────────────────────────────────────────────────────

/// Internal state machine for model lifecycle.
pub(crate) enum ModelState {
    /// Model has not been downloaded or loaded.
    Unloaded,

    /// Model weights are being downloaded.
    Downloading,

    /// Model is being loaded into the inference engine.
    Loading,

    /// Model is ready for inference.
    Ready { runner: mistralrs::Model },

    /// Model failed to load.
    Failed { error: String },
}

impl std::fmt::Debug for ModelState {
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
    state: RwLock<ModelState>,
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
                state: RwLock::new(ModelState::Unloaded),
                ready_notify: Notify::new(),
                config,
                progress_cb: None,
            }),
        }
    }

    /// Builder method: attach a progress callback.
    pub fn with_progress(mut self, cb: ProgressCallbackFn) -> Result<Self, LocalModelError> {
        let inner = Arc::get_mut(&mut self.inner)
            .ok_or_else(|| LocalModelError::inference("with_progress called after clone — Arc is shared"))?;
        inner.progress_cb = Some(cb);
        Ok(self)
    }

    /// Returns `true` if the model is loaded and ready for inference.
    pub async fn is_ready(&self) -> bool {
        matches!(*self.inner.state.read().await, ModelState::Ready { .. })
    }

    /// Block until the model reaches the `Ready` state.
    ///
    /// Uses the same `Notify` pattern as `agent.rs::wait_for_idle()`.
    pub async fn wait_until_ready(&self) {
        loop {
            if self.is_ready().await {
                return;
            }
            self.inner.ready_notify.notified().await;
        }
    }

    /// Idempotent: download → load → ready.
    ///
    /// Concurrent callers serialize on the `RwLock` — only the first caller
    /// triggers the download/load sequence; others wait for completion.
    pub async fn ensure_ready(&self) -> Result<(), LocalModelError> {
        // Fast path: already ready.
        {
            let state = self.inner.state.read().await;
            match &*state {
                ModelState::Ready { .. } => return Ok(()),
                ModelState::Failed { error } => {
                    return Err(LocalModelError::Loading {
                        source: error.clone().into(),
                    });
                }
                ModelState::Downloading | ModelState::Loading => {
                    drop(state);
                    self.wait_until_ready().await;
                    return Ok(());
                }
                ModelState::Unloaded => {}
            }
        }

        // Slow path: acquire write lock and load.
        let mut state = self.inner.state.write().await;

        // Double-check after acquiring write lock.
        match &*state {
            ModelState::Ready { .. } => return Ok(()),
            ModelState::Failed { error } => {
                return Err(LocalModelError::Loading {
                    source: error.clone().into(),
                });
            }
            ModelState::Downloading | ModelState::Loading => {
                // Another task is loading — shouldn't happen with write lock,
                // but handle gracefully.
                drop(state);
                self.wait_until_ready().await;
                return Ok(());
            }
            ModelState::Unloaded => {}
        }

        // Begin loading.
        *state = ModelState::Downloading;
        self.notify_progress(ModelProgress::Downloading {
            downloaded: 0,
            total: 0,
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
            self.notify_progress(ModelProgress::Failed { message: msg.clone() });
            *state = ModelState::Failed { error: msg };
            self.inner.ready_notify.notify_waiters();
            LocalModelError::download(e)
        })?;

        let repo = api.model(self.inner.config.repo_id.clone());
        let model_path = repo.get(&self.inner.config.filename).await.map_err(|e| {
            let msg = format!("model download failed: {e}");
            error!(%msg);
            self.notify_progress(ModelProgress::Failed { message: msg.clone() });
            *state = ModelState::Failed { error: msg };
            self.inner.ready_notify.notify_waiters();
            LocalModelError::download(e)
        })?;

        debug!(path = %model_path.display(), "model downloaded, loading");

        *state = ModelState::Loading;
        self.notify_progress(ModelProgress::Loading);

        // Build the GGUF model via mistral.rs.
        let runner = mistralrs::GgufModelBuilder::new(
            self.inner.config.repo_id.clone(),
            vec![self.inner.config.filename.clone()],
        )
        .build()
        .await
        .map_err(|e| {
            let msg = format!("model loading failed: {e}");
            error!(%msg);
            self.notify_progress(ModelProgress::Failed { message: msg.clone() });
            *state = ModelState::Failed { error: msg.clone() };
            self.inner.ready_notify.notify_waiters();
            LocalModelError::Loading {
                source: msg.into(),
            }
        })?;

        info!("local model ready");
        *state = ModelState::Ready { runner };
        drop(state);
        self.notify_progress(ModelProgress::Ready);
        self.inner.ready_notify.notify_waiters();

        Ok(())
    }

    /// Drop the loaded model, returning to `Unloaded` state.
    pub async fn unload(&self) {
        let mut state = self.inner.state.write().await;
        *state = ModelState::Unloaded;
        drop(state);
        info!("local model unloaded");
    }

    /// Get a reference to the underlying mistral.rs `Model` runner.
    ///
    /// Returns `Err(NotReady)` if the model is not loaded.
    pub(crate) async fn runner(
        &self,
    ) -> Result<tokio::sync::RwLockReadGuard<'_, ModelState>, LocalModelError> {
        let state = self.inner.state.read().await;
        if matches!(&*state, ModelState::Ready { .. }) {
            Ok(state)
        } else {
            Err(LocalModelError::NotReady)
        }
    }

    /// Access the model configuration.
    pub fn config(&self) -> &ModelConfig {
        &self.inner.config
    }

    fn notify_progress(&self, progress: ModelProgress) {
        if let Some(cb) = &self.inner.progress_cb {
            cb(progress);
        }
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
    async fn runner_returns_not_ready_when_unloaded() {
        let model = LocalModel::new(ModelConfig::default());
        let result = model.runner().await;
        assert!(result.is_err());
    }

    #[test]
    fn model_state_debug() {
        let states = [
            ModelState::Unloaded,
            ModelState::Downloading,
            ModelState::Loading,
            ModelState::Failed {
                error: "test".into(),
            },
        ];
        for s in &states {
            let debug = format!("{s:?}");
            assert!(!debug.is_empty());
        }
    }
}
