//! Local embedding model for text vectorization.
//!
//! [`EmbeddingModel`] wraps a mistral.rs embedding model behind `Arc` for
//! cheap cloning and concurrent access. Lazily downloaded from `HuggingFace`
//! on first use, same pattern as [`LocalModel`](crate::model::LocalModel).

use std::sync::Arc;

use tokio::sync::{Notify, RwLock};
use tracing::{debug, error, info};

use crate::error::LocalModelError;
use crate::progress::{ModelProgress, ProgressCallbackFn};

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
            filename: std::env::var("LOCAL_EMBED_FILE")
                .unwrap_or_else(|_| String::new()),
            dimensions: std::env::var("LOCAL_EMBED_DIMS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(768),
        }
    }
}

// ─── EmbeddingModelState ────────────────────────────────────────────────────

/// Internal state machine for embedding model lifecycle.
enum EmbeddingModelState {
    Unloaded,
    Downloading,
    Loading,
    Ready { runner: mistralrs::Model },
    Failed { error: String },
}

impl std::fmt::Debug for EmbeddingModelState {
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

// ─── EmbeddingModel ─────────────────────────────────────────────────────────

/// A lazily-loaded local embedding model for text vectorization.
///
/// Wraps `Arc<EmbeddingModelInner>` for cheap cloning — multiple tasks can
/// share the same loaded model concurrently.
#[derive(Clone)]
pub struct EmbeddingModel {
    inner: Arc<EmbeddingModelInner>,
}

struct EmbeddingModelInner {
    state: RwLock<EmbeddingModelState>,
    ready_notify: Notify,
    config: EmbeddingConfig,
    progress_cb: Option<ProgressCallbackFn>,
}

impl std::fmt::Debug for EmbeddingModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbeddingModel")
            .field("config", &self.inner.config)
            .finish_non_exhaustive()
    }
}

impl EmbeddingModel {
    /// Create a new `EmbeddingModel` in the `Unloaded` state.
    #[must_use]
    pub fn new(config: EmbeddingConfig) -> Self {
        Self {
            inner: Arc::new(EmbeddingModelInner {
                state: RwLock::new(EmbeddingModelState::Unloaded),
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

    /// Returns `true` if the model is loaded and ready.
    pub async fn is_ready(&self) -> bool {
        matches!(
            *self.inner.state.read().await,
            EmbeddingModelState::Ready { .. }
        )
    }

    /// Idempotent: download → load → ready.
    pub async fn ensure_ready(&self) -> Result<(), LocalModelError> {
        // Fast path.
        {
            let state = self.inner.state.read().await;
            match &*state {
                EmbeddingModelState::Ready { .. } => return Ok(()),
                EmbeddingModelState::Failed { error } => {
                    return Err(LocalModelError::Loading {
                        source: error.clone().into(),
                    });
                }
                EmbeddingModelState::Downloading | EmbeddingModelState::Loading => {
                    drop(state);
                    self.wait_until_ready().await;
                    return Ok(());
                }
                EmbeddingModelState::Unloaded => {}
            }
        }

        // Slow path: acquire write lock.
        let mut state = self.inner.state.write().await;

        // Double-check.
        match &*state {
            EmbeddingModelState::Ready { .. } => return Ok(()),
            EmbeddingModelState::Failed { error } => {
                return Err(LocalModelError::Loading {
                    source: error.clone().into(),
                });
            }
            EmbeddingModelState::Downloading | EmbeddingModelState::Loading => {
                drop(state);
                self.wait_until_ready().await;
                return Ok(());
            }
            EmbeddingModelState::Unloaded => {}
        }

        *state = EmbeddingModelState::Downloading;
        self.notify_progress(ModelProgress::Downloading {
            downloaded: 0,
            total: 0,
        });

        info!(
            repo = %self.inner.config.repo_id,
            "downloading embedding model"
        );

        // Build the embedding model via mistral.rs.
        *state = EmbeddingModelState::Loading;
        self.notify_progress(ModelProgress::Loading);

        let runner = mistralrs::EmbeddingModelBuilder::new(
            self.inner.config.repo_id.clone(),
        )
        .build()
        .await
        .map_err(|e| {
            let msg = format!("embedding model loading failed: {e}");
            error!(%msg);
            self.notify_progress(ModelProgress::Failed {
                message: msg.clone(),
            });
            *state = EmbeddingModelState::Failed { error: msg.clone() };
            self.inner.ready_notify.notify_waiters();
            LocalModelError::Loading {
                source: msg.into(),
            }
        })?;

        info!("embedding model ready");
        *state = EmbeddingModelState::Ready { runner };
        drop(state);
        self.notify_progress(ModelProgress::Ready);
        self.inner.ready_notify.notify_waiters();

        Ok(())
    }

    /// Embed a batch of texts, returning one vector per input.
    pub async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, LocalModelError> {
        self.ensure_ready().await?;

        let state = self.inner.state.read().await;
        let EmbeddingModelState::Ready { runner } = &*state else {
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
            .map_err(|e| LocalModelError::inference(format!("embedding failed: {e}")))?;

        drop(state);
        Ok(embeddings)
    }

    /// Convenience method: embed a single text.
    pub async fn embed_single(&self, text: &str) -> Result<Vec<f32>, LocalModelError> {
        self.ensure_ready().await?;

        let state = self.inner.state.read().await;
        let EmbeddingModelState::Ready { runner } = &*state else {
            return Err(LocalModelError::NotReady);
        };

        let result = runner
            .generate_embedding(text)
            .await
            .map_err(|e| LocalModelError::inference(format!("embedding failed: {e}")));
        drop(state);
        result
    }

    /// Drop the loaded model, returning to `Unloaded` state.
    pub async fn unload(&self) {
        let mut state = self.inner.state.write().await;
        *state = EmbeddingModelState::Unloaded;
        drop(state);
        info!("embedding model unloaded");
    }

    /// Access the configuration.
    pub fn config(&self) -> &EmbeddingConfig {
        &self.inner.config
    }

    async fn wait_until_ready(&self) {
        loop {
            if self.is_ready().await {
                return;
            }
            self.inner.ready_notify.notified().await;
        }
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
    assert_send_sync::<EmbeddingModel>();
    assert_send_sync::<EmbeddingConfig>();
};

#[cfg(test)]
mod tests {
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

}
