//! Tests for `loader`.
#![cfg(test)]

use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use tokio::time::timeout;

use super::*;

#[derive(Debug)]
struct FailingConfig {
    download_attempts: Arc<AtomicUsize>,
}

struct FailingBackend;

impl LoaderBackend for FailingBackend {
    type Config = FailingConfig;
    type Artifact = ();
    type Runner = ();

    fn download(
        config: &Self::Config,
        _progress_cb: Option<ProgressCallbackFn>,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Artifact, LocalModelError>> + Send + '_>> {
        config.download_attempts.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Err(LocalModelError::download(io::Error::other(
                "synthetic download failure",
            )))
        })
    }

    fn build(
        _config: &Self::Config,
        _artifact: Self::Artifact,
        _progress_cb: Option<ProgressCallbackFn>,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Runner, LocalModelError>> + Send + '_>> {
        Box::pin(async { unreachable!("failing backend never builds") })
    }

    fn label() -> &'static str {
        "failing test backend"
    }
}

#[derive(Debug)]
struct FlakyConfig {
    download_attempts: Arc<AtomicUsize>,
}

struct FlakyBackend;

impl LoaderBackend for FlakyBackend {
    type Config = FlakyConfig;
    type Artifact = ();
    type Runner = ();

    fn download(
        config: &Self::Config,
        _progress_cb: Option<ProgressCallbackFn>,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Artifact, LocalModelError>> + Send + '_>> {
        let attempt = config.download_attempts.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move {
            if attempt == 1 {
                Err(LocalModelError::download(io::Error::other(
                    "synthetic transient failure",
                )))
            } else {
                Ok(())
            }
        })
    }

    fn build(
        _config: &Self::Config,
        _artifact: Self::Artifact,
        _progress_cb: Option<ProgressCallbackFn>,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Runner, LocalModelError>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    fn label() -> &'static str {
        "flaky test backend"
    }
}

#[derive(Debug)]
struct BlockingConfig {
    download_attempts: Arc<AtomicUsize>,
    build_attempts: Arc<AtomicUsize>,
    block_download_once: Arc<AtomicBool>,
    block_build_once: Arc<AtomicBool>,
    download_started: Arc<Notify>,
    build_started: Arc<Notify>,
    release_download: Arc<Notify>,
    release_build: Arc<Notify>,
}

impl BlockingConfig {
    fn new() -> Self {
        Self {
            download_attempts: Arc::new(AtomicUsize::new(0)),
            build_attempts: Arc::new(AtomicUsize::new(0)),
            block_download_once: Arc::new(AtomicBool::new(false)),
            block_build_once: Arc::new(AtomicBool::new(false)),
            download_started: Arc::new(Notify::new()),
            build_started: Arc::new(Notify::new()),
            release_download: Arc::new(Notify::new()),
            release_build: Arc::new(Notify::new()),
        }
    }
}

struct BlockingBackend;

impl LoaderBackend for BlockingBackend {
    type Config = BlockingConfig;
    type Artifact = ();
    type Runner = ();

    fn download(
        config: &Self::Config,
        _progress_cb: Option<ProgressCallbackFn>,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Artifact, LocalModelError>> + Send + '_>> {
        config.download_attempts.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if config.block_download_once.swap(false, Ordering::SeqCst) {
                config.download_started.notify_waiters();
                config.release_download.notified().await;
            }
            Ok(())
        })
    }

    fn build(
        config: &Self::Config,
        _artifact: Self::Artifact,
        _progress_cb: Option<ProgressCallbackFn>,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Runner, LocalModelError>> + Send + '_>> {
        config.build_attempts.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if config.block_build_once.swap(false, Ordering::SeqCst) {
                config.build_started.notify_waiters();
                config.release_build.notified().await;
            }
            Ok(())
        })
    }

    fn label() -> &'static str {
        "blocking test backend"
    }
}

#[test]
fn loader_state_debug() {
    let states: Vec<LoaderState<()>> = vec![
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
        classify::<()>(&LoaderState::Unloaded),
        StateClass::Unloaded
    ));
    assert!(matches!(
        classify::<()>(&LoaderState::Downloading),
        StateClass::Waiting
    ));
    assert!(matches!(
        classify::<()>(&LoaderState::Loading),
        StateClass::Waiting
    ));
    assert!(matches!(
        classify::<()>(&LoaderState::Failed { error: "e".into() }),
        StateClass::Failed
    ));
}

#[tokio::test]
async fn wait_until_ready_returns_when_unload_resets_loader() {
    let loader = LazyLoader::<FailingBackend>::new(FailingConfig {
        download_attempts: Arc::new(AtomicUsize::new(0)),
    });

    {
        let mut state = loader.inner.state.write().await;
        *state = LoaderState::Downloading;
    }

    let waiting_loader = loader.clone();
    let waiter = tokio::spawn(async move {
        timeout(Duration::from_secs(1), waiting_loader.wait_until_ready()).await
    });

    tokio::task::yield_now().await;
    loader.unload().await;

    let result = waiter.await.expect("wait task should join");
    assert!(result.is_ok(), "wait_until_ready() timed out after unload");
    assert_eq!(loader.public_state().await, PublicLoaderState::Unloaded);
}

#[tokio::test]
async fn wait_until_ready_returns_when_loading_fails() {
    let loader = LazyLoader::<FailingBackend>::new(FailingConfig {
        download_attempts: Arc::new(AtomicUsize::new(0)),
    });

    {
        let mut state = loader.inner.state.write().await;
        *state = LoaderState::Loading;
    }

    let waiting_loader = loader.clone();
    let waiter = tokio::spawn(async move {
        timeout(Duration::from_secs(1), waiting_loader.wait_until_ready()).await
    });

    tokio::task::yield_now().await;

    {
        let mut state = loader.inner.state.write().await;
        *state = LoaderState::Failed {
            error: "synthetic failure".into(),
        };
    }
    loader.inner.ready_notify.notify_waiters();

    let result = waiter.await.expect("wait task should join");
    assert!(result.is_ok(), "wait_until_ready() timed out after failure");
}

#[tokio::test]
async fn ensure_ready_retries_after_unload_wakes_waiter() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let loader = LazyLoader::<FailingBackend>::new(FailingConfig {
        download_attempts: Arc::clone(&attempts),
    });

    {
        let mut state = loader.inner.state.write().await;
        *state = LoaderState::Downloading;
    }

    let waiting_loader = loader.clone();
    let ensure = tokio::spawn(async move { waiting_loader.ensure_ready().await });

    tokio::task::yield_now().await;
    loader.unload().await;

    let err = ensure.await.expect("ensure task should join").unwrap_err();
    assert!(
        err.to_string().contains("synthetic download failure"),
        "unexpected error: {err}"
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert!(matches!(
        &*loader.inner.state.read().await,
        LoaderState::Failed { .. }
    ));
}

#[tokio::test]
async fn ensure_ready_retries_after_failed_state() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let loader = LazyLoader::<FlakyBackend>::new(FlakyConfig {
        download_attempts: Arc::clone(&attempts),
    });

    let err = loader.ensure_ready().await.unwrap_err();
    assert!(
        err.to_string().contains("synthetic transient failure"),
        "unexpected error: {err}"
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert!(matches!(
        &*loader.inner.state.read().await,
        LoaderState::Failed { .. }
    ));

    loader
        .ensure_ready()
        .await
        .expect("failed state should retry and recover");

    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert!(matches!(
        &*loader.inner.state.read().await,
        LoaderState::Ready { .. }
    ));
}

#[tokio::test]
async fn cancelling_download_wakes_waiters_and_allows_retry() {
    let config = BlockingConfig::new();
    config.block_download_once.store(true, Ordering::SeqCst);
    let download_started = Arc::clone(&config.download_started);
    let attempts = Arc::clone(&config.download_attempts);
    let loader = LazyLoader::<BlockingBackend>::new(config);

    let waiting_loader = loader.clone();
    let waiter = tokio::spawn(async move {
        timeout(Duration::from_secs(1), waiting_loader.wait_until_ready()).await
    });

    let loading_loader = loader.clone();
    let load = tokio::spawn(async move { loading_loader.ensure_ready().await });
    timeout(Duration::from_secs(1), download_started.notified())
        .await
        .expect("download phase should start");

    load.abort();
    assert!(
        load.await
            .expect_err("load task should be aborted")
            .is_cancelled()
    );

    let result = waiter.await.expect("wait task should join");
    assert!(
        result.is_ok(),
        "cancelled download must notify readiness waiters"
    );
    assert!(matches!(
        &*loader.inner.state.read().await,
        LoaderState::Failed { error } if error.contains("cancelled")
    ));

    loader
        .ensure_ready()
        .await
        .expect("cancelled download state should allow retry");
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(loader.public_state().await, PublicLoaderState::Ready);
}

#[tokio::test]
async fn cancelling_build_wakes_waiters_and_allows_retry() {
    let config = BlockingConfig::new();
    config.block_build_once.store(true, Ordering::SeqCst);
    let build_started = Arc::clone(&config.build_started);
    let builds = Arc::clone(&config.build_attempts);
    let loader = LazyLoader::<BlockingBackend>::new(config);

    let waiting_loader = loader.clone();
    let waiter = tokio::spawn(async move {
        timeout(Duration::from_secs(1), waiting_loader.wait_until_ready()).await
    });

    let loading_loader = loader.clone();
    let load = tokio::spawn(async move { loading_loader.ensure_ready().await });
    timeout(Duration::from_secs(1), build_started.notified())
        .await
        .expect("build phase should start");

    load.abort();
    assert!(
        load.await
            .expect_err("load task should be aborted")
            .is_cancelled()
    );

    let result = waiter.await.expect("wait task should join");
    assert!(
        result.is_ok(),
        "cancelled build must notify readiness waiters"
    );
    assert!(matches!(
        &*loader.inner.state.read().await,
        LoaderState::Failed { error } if error.contains("cancelled")
    ));

    loader
        .ensure_ready()
        .await
        .expect("cancelled build state should allow retry");
    assert_eq!(builds.load(Ordering::SeqCst), 2);
    assert_eq!(loader.public_state().await, PublicLoaderState::Ready);
}
