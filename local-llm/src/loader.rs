//! Shared lazy-loader core for local models.
//!
//! [`LazyLoader`] owns the lifecycle state machine
//! (Unloaded → Downloading → Loading → Ready / Failed), progress callbacks,
//! and `Notify`-based readiness signalling. Backend-specific download and
//! build logic is injected via the [`LoaderBackend`] trait.
//!
//! Both [`crate::model::LocalModel`] and [`crate::embedding::EmbeddingModel`]
//! are thin typed wrappers over `LazyLoader`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::{Notify, RwLock};
use tracing::{error, info};

use crate::error::LocalModelError;
use crate::progress::{ProgressCallbackFn, ProgressEvent};

// ─── LoaderState ───────────────────────────────────────────────────────────

/// Internal lifecycle state with the runner reference.
pub enum LoaderState {
    Unloaded,
    Downloading,
    Loading,
    Ready { runner: mistralrs::Model },
    Failed { error: String },
}

impl std::fmt::Debug for LoaderState {
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

// ─── LoaderBackend trait ───────────────────────────────────────────────────

/// Backend-specific download and build logic for a lazy-loaded model.
///
/// Implementors provide two phases:
/// 1. **Download** — fetch model artifacts (returns an intermediate value).
/// 2. **Build** — load the downloaded artifact into a `mistralrs::Model`.
///
/// The [`LazyLoader`] drives the state machine around these two phases.
pub trait LoaderBackend: Send + Sync + 'static {
    /// Configuration type (e.g. `ModelConfig`, `EmbeddingConfig`).
    type Config: std::fmt::Debug + Send + Sync + 'static;

    /// Intermediate artifact produced by `download` and consumed by `build`.
    type Artifact: Send + 'static;

    /// Download model artifacts. Called while state is `Downloading`.
    ///
    /// Receives the config and an optional progress callback for emitting
    /// download progress events.
    fn download(
        config: &Self::Config,
        progress_cb: Option<ProgressCallbackFn>,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Artifact, LocalModelError>> + Send + '_>>;

    /// Build the model from downloaded artifacts. Called while state is `Loading`.
    fn build(
        config: &Self::Config,
        artifact: Self::Artifact,
        progress_cb: Option<ProgressCallbackFn>,
    ) -> Pin<Box<dyn Future<Output = Result<mistralrs::Model, LocalModelError>> + Send + '_>>;

    /// Human-readable label for log messages (e.g. "local model", "embedding model").
    fn label() -> &'static str;
}

// ─── LazyLoader ────────────────────────────────────────────────────────────

/// A lazily-loaded local model backed by a [`LoaderBackend`].
///
/// Wraps `Arc<Inner>` for cheap cloning — multiple tasks can share the same
/// loaded model concurrently.
pub struct LazyLoader<B: LoaderBackend> {
    inner: Arc<LazyLoaderInner<B>>,
}

struct LazyLoaderInner<B: LoaderBackend> {
    state: RwLock<LoaderState>,
    ready_notify: Notify,
    config: B::Config,
    progress_cb: Option<ProgressCallbackFn>,
}

impl<B: LoaderBackend> Clone for LazyLoader<B> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<B: LoaderBackend> std::fmt::Debug for LazyLoader<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LazyLoader")
            .field("config", &self.inner.config)
            .finish_non_exhaustive()
    }
}

impl<B: LoaderBackend> LazyLoader<B> {
    /// Create a new loader in the `Unloaded` state.
    pub fn new(config: B::Config) -> Self {
        Self {
            inner: Arc::new(LazyLoaderInner {
                state: RwLock::new(LoaderState::Unloaded),
                ready_notify: Notify::new(),
                config,
                progress_cb: None,
            }),
        }
    }

    /// Attach a progress callback. Must be called before cloning.
    pub fn with_progress(mut self, cb: ProgressCallbackFn) -> Result<Self, LocalModelError> {
        let inner = Arc::get_mut(&mut self.inner).ok_or_else(|| {
            LocalModelError::inference("with_progress called after clone — Arc is shared")
        })?;
        inner.progress_cb = Some(cb);
        Ok(self)
    }

    /// Returns `true` if the model is loaded and ready.
    pub async fn is_ready(&self) -> bool {
        matches!(*self.inner.state.read().await, LoaderState::Ready { .. })
    }

    /// Block until the model reaches the `Ready` state.
    pub async fn wait_until_ready(&self) {
        loop {
            if self.is_ready().await {
                return;
            }
            self.inner.ready_notify.notified().await;
        }
    }

    /// Access the configuration.
    pub fn config(&self) -> &B::Config {
        &self.inner.config
    }

    /// Emit a progress event via the stored callback.
    fn notify_progress(&self, progress: ProgressEvent) {
        if let Some(cb) = &self.inner.progress_cb {
            cb(progress);
        }
    }

    /// Idempotent: download → load → ready.
    ///
    /// Concurrent callers serialize on the `RwLock` — only the first caller
    /// triggers the download/load sequence; others wait for completion.
    pub async fn ensure_ready(&self) -> Result<(), LocalModelError> {
        // Fast path: already ready (read lock only).
        {
            let state = self.inner.state.read().await;
            match classify(&state) {
                StateClass::Ready => return Ok(()),
                StateClass::Failed(error) => {
                    return Err(LocalModelError::loading_message(error));
                }
                StateClass::Waiting => {
                    drop(state);
                    self.wait_until_ready().await;
                    return Ok(());
                }
                StateClass::Unloaded => {}
            }
        }

        // Slow path: acquire write lock.
        let mut state = self.inner.state.write().await;

        // Double-check after acquiring write lock.
        match classify(&state) {
            StateClass::Ready => return Ok(()),
            StateClass::Failed(error) => {
                return Err(LocalModelError::loading_message(error));
            }
            StateClass::Waiting => {
                drop(state);
                self.wait_until_ready().await;
                return Ok(());
            }
            StateClass::Unloaded => {}
        }

        // ── Phase 1: Download ──────────────────────────────────────────
        *state = LoaderState::Downloading;
        self.notify_progress(ProgressEvent::DownloadProgress {
            bytes_downloaded: 0,
            total_bytes: None,
        });

        let artifact = match B::download(&self.inner.config, self.inner.progress_cb.clone()).await {
            Ok(a) => a,
            Err(e) => {
                error!(error = %e, "{} download failed", B::label());
                *state = LoaderState::Failed {
                    error: e.to_string(),
                };
                self.inner.ready_notify.notify_waiters();
                return Err(e);
            }
        };

        self.notify_progress(ProgressEvent::DownloadComplete);

        // ── Phase 2: Build ─────────────────────────────────────────────
        *state = LoaderState::Loading;
        self.notify_progress(ProgressEvent::LoadingProgress {
            message: format!("loading {}", B::label()),
        });

        let runner =
            match B::build(&self.inner.config, artifact, self.inner.progress_cb.clone()).await {
                Ok(r) => r,
                Err(e) => {
                    error!(error = %e, "{} loading failed", B::label());
                    *state = LoaderState::Failed {
                        error: e.to_string(),
                    };
                    self.inner.ready_notify.notify_waiters();
                    return Err(e);
                }
            };

        info!("{} ready", B::label());
        *state = LoaderState::Ready { runner };
        drop(state);
        self.notify_progress(ProgressEvent::LoadingComplete);
        self.inner.ready_notify.notify_waiters();

        Ok(())
    }

    /// Get a read guard over the internal state. Returns `Err(NotReady)` if
    /// the model is not in the `Ready` state.
    pub async fn runner(
        &self,
    ) -> Result<tokio::sync::RwLockReadGuard<'_, LoaderState>, LocalModelError> {
        let state = self.inner.state.read().await;
        if matches!(&*state, LoaderState::Ready { .. }) {
            Ok(state)
        } else {
            Err(LocalModelError::NotReady)
        }
    }

    /// Drop the loaded model, returning to `Unloaded` state.
    pub async fn unload(&self) {
        let mut state = self.inner.state.write().await;
        *state = LoaderState::Unloaded;
        drop(state);
        info!("{} unloaded", B::label());
    }

    /// Returns the current public-facing state.
    pub async fn public_state(&self) -> PublicLoaderState {
        match &*self.inner.state.read().await {
            LoaderState::Unloaded => PublicLoaderState::Unloaded,
            LoaderState::Downloading => PublicLoaderState::Downloading,
            LoaderState::Loading => PublicLoaderState::Loading,
            LoaderState::Ready { .. } => PublicLoaderState::Ready,
            LoaderState::Failed { error } => PublicLoaderState::Failed(error.clone()),
        }
    }
}

// ─── PublicLoaderState ─────────────────────────────────────────────────────

/// Public-facing lifecycle state (without the runner reference).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicLoaderState {
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

// ─── Helpers ───────────────────────────────────────────────────────────────

enum StateClass {
    Ready,
    Failed(String),
    Waiting,
    Unloaded,
}

fn classify(state: &LoaderState) -> StateClass {
    match state {
        LoaderState::Ready { .. } => StateClass::Ready,
        LoaderState::Failed { error } => StateClass::Failed(error.clone()),
        LoaderState::Downloading | LoaderState::Loading => StateClass::Waiting,
        LoaderState::Unloaded => StateClass::Unloaded,
    }
}

// ─── Compile-time assertions ───────────────────────────────────────────────

const _: () = {
    // LazyLoader<B> is Send + Sync when B is Send + Sync (always true
    // since LoaderBackend requires Send + Sync + 'static).
    // Concrete assertions are in model.rs and embedding.rs.
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loader_state_debug() {
        let states = [
            LoaderState::Unloaded,
            LoaderState::Downloading,
            LoaderState::Loading,
            LoaderState::Failed {
                error: "test".into(),
            },
        ];
        for s in &states {
            let debug = format!("{s:?}");
            assert!(!debug.is_empty());
        }
    }

    #[test]
    fn public_loader_state_eq() {
        assert_eq!(PublicLoaderState::Unloaded, PublicLoaderState::Unloaded);
        assert_eq!(PublicLoaderState::Ready, PublicLoaderState::Ready);
        assert_eq!(
            PublicLoaderState::Failed("x".into()),
            PublicLoaderState::Failed("x".into())
        );
        assert_ne!(PublicLoaderState::Unloaded, PublicLoaderState::Ready);
    }

    #[test]
    fn classify_states() {
        assert!(matches!(
            classify(&LoaderState::Unloaded),
            StateClass::Unloaded
        ));
        assert!(matches!(
            classify(&LoaderState::Downloading),
            StateClass::Waiting
        ));
        assert!(matches!(
            classify(&LoaderState::Loading),
            StateClass::Waiting
        ));
        assert!(matches!(
            classify(&LoaderState::Failed { error: "e".into() }),
            StateClass::Failed(_)
        ));
    }
}
