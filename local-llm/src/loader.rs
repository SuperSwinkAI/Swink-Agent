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

use tokio::sync::{Notify, RwLock, RwLockWriteGuard};
use tracing::{error, info};

use crate::error::LocalModelError;
use crate::progress::{ProgressCallbackFn, ProgressEvent};

// ─── LoaderState ───────────────────────────────────────────────────────────

/// Internal lifecycle state, generic over the runner type `R`.
pub enum LoaderState<R> {
    Unloaded,
    Downloading,
    Loading,
    Ready { runner: R },
    Failed { error: String },
}

impl<R> std::fmt::Debug for LoaderState<R> {
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
/// 2. **Build** — load the downloaded artifact into a runner.
///
/// The [`LazyLoader`] drives the state machine around these two phases.
pub trait LoaderBackend: Send + Sync + 'static {
    /// Configuration type (e.g. `ModelConfig`, `EmbeddingConfig`).
    type Config: std::fmt::Debug + Send + Sync + 'static;

    /// Intermediate artifact produced by `download` and consumed by `build`.
    type Artifact: Send + 'static;

    /// The runner type stored in `LoaderState::Ready`.
    type Runner: Send + Sync + 'static;

    /// Download model artifacts. Called while state is `Downloading`.
    fn download(
        config: &Self::Config,
        progress_cb: Option<ProgressCallbackFn>,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Artifact, LocalModelError>> + Send + '_>>;

    /// Build the runner from downloaded artifacts. Called while state is `Loading`.
    fn build(
        config: &Self::Config,
        artifact: Self::Artifact,
        progress_cb: Option<ProgressCallbackFn>,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Runner, LocalModelError>> + Send + '_>>;

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
    state: RwLock<LoaderState<B::Runner>>,
    ready_notify: Notify,
    config: B::Config,
    progress_cb: Option<ProgressCallbackFn>,
}

struct LoadAttemptGuard<'a, R> {
    state: RwLockWriteGuard<'a, LoaderState<R>>,
    ready_notify: &'a Notify,
    label: &'static str,
    finished: bool,
}

impl<'a, R> LoadAttemptGuard<'a, R> {
    fn new(
        state: RwLockWriteGuard<'a, LoaderState<R>>,
        ready_notify: &'a Notify,
        label: &'static str,
    ) -> Self {
        Self {
            state,
            ready_notify,
            label,
            finished: false,
        }
    }

    fn set_downloading(&mut self) {
        *self.state = LoaderState::Downloading;
    }

    fn set_loading(&mut self) {
        *self.state = LoaderState::Loading;
    }

    fn fail(&mut self, error: String) {
        *self.state = LoaderState::Failed { error };
        self.finished = true;
        self.ready_notify.notify_waiters();
    }

    fn ready(mut self, runner: R) {
        *self.state = LoaderState::Ready { runner };
        self.finished = true;
    }
}

impl<R> Drop for LoadAttemptGuard<'_, R> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }

        *self.state = LoaderState::Failed {
            error: format!("{} load attempt cancelled", self.label),
        };
        self.ready_notify.notify_waiters();
    }
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

    /// Block until the current load attempt reaches a terminal state.
    pub async fn wait_until_ready(&self) {
        loop {
            let notified = {
                let state = self.inner.state.read().await;
                match classify(&state) {
                    StateClass::Ready | StateClass::Failed | StateClass::Unloaded => return,
                    StateClass::Waiting => self.inner.ready_notify.notified(),
                }
            };

            notified.await;
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
    pub async fn ensure_ready(&self) -> Result<(), LocalModelError> {
        loop {
            {
                let state = self.inner.state.read().await;
                match classify(&state) {
                    StateClass::Ready => return Ok(()),
                    StateClass::Waiting => {
                        drop(state);
                        self.wait_until_ready().await;
                        continue;
                    }
                    StateClass::Failed | StateClass::Unloaded => {}
                }
            }

            let state = self.inner.state.write().await;

            match classify(&state) {
                StateClass::Ready => return Ok(()),
                StateClass::Waiting => {
                    drop(state);
                    self.wait_until_ready().await;
                    continue;
                }
                StateClass::Failed | StateClass::Unloaded => {}
            }

            let mut attempt = LoadAttemptGuard::new(state, &self.inner.ready_notify, B::label());

            // ── Phase 1: Download ──────────────────────────────────────────
            attempt.set_downloading();
            self.notify_progress(ProgressEvent::DownloadProgress {
                bytes_downloaded: 0,
                total_bytes: None,
            });

            let artifact =
                match B::download(&self.inner.config, self.inner.progress_cb.clone()).await {
                    Ok(a) => a,
                    Err(e) => {
                        error!(error = %e, "{} download failed", B::label());
                        attempt.fail(e.to_string());
                        return Err(e);
                    }
                };

            self.notify_progress(ProgressEvent::DownloadComplete);

            // ── Phase 2: Build ─────────────────────────────────────────────
            attempt.set_loading();
            self.notify_progress(ProgressEvent::LoadingProgress {
                message: format!("loading {}", B::label()),
            });

            let runner = match B::build(
                &self.inner.config,
                artifact,
                self.inner.progress_cb.clone(),
            )
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    error!(error = %e, "{} loading failed", B::label());
                    attempt.fail(e.to_string());
                    return Err(e);
                }
            };

            info!("{} ready", B::label());
            attempt.ready(runner);
            self.notify_progress(ProgressEvent::LoadingComplete);
            self.inner.ready_notify.notify_waiters();

            return Ok(());
        }
    }

    /// Get a read guard over the internal state. Returns `Err(NotReady)` if
    /// the model is not in the `Ready` state.
    pub async fn runner(
        &self,
    ) -> Result<tokio::sync::RwLockReadGuard<'_, LoaderState<B::Runner>>, LocalModelError> {
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
        self.inner.ready_notify.notify_waiters();
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
    Unloaded,
    Downloading,
    Loading,
    Ready,
    Failed(String),
}

// ─── Helpers ───────────────────────────────────────────────────────────────

enum StateClass {
    Ready,
    Failed,
    Waiting,
    Unloaded,
}

fn classify<R>(state: &LoaderState<R>) -> StateClass {
    match state {
        LoaderState::Ready { .. } => StateClass::Ready,
        LoaderState::Failed { .. } => StateClass::Failed,
        LoaderState::Downloading | LoaderState::Loading => StateClass::Waiting,
        LoaderState::Unloaded => StateClass::Unloaded,
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "loader_tests.rs"]
mod tests;
