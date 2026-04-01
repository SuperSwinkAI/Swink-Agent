use std::future::Future;
use std::sync::Arc;

use tokio::sync::Notify;

use crate::error::LocalModelError;
use crate::progress::{ProgressCallbackFn, ProgressEvent};

pub enum LoadStateCheck {
    Ready,
    Waiting,
    Failed(String),
    Unloaded,
}

pub fn attach_progress_callback<T>(
    inner: &mut Arc<T>,
    cb: ProgressCallbackFn,
    set: impl FnOnce(&mut T, ProgressCallbackFn),
) -> Result<(), LocalModelError> {
    let inner = Arc::get_mut(inner).ok_or_else(|| {
        LocalModelError::inference("with_progress called after clone — Arc is shared")
    })?;
    set(inner, cb);
    Ok(())
}

pub async fn wait_until_ready<F, Fut>(notify: &Notify, mut is_ready: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    loop {
        if is_ready().await {
            return;
        }
        notify.notified().await;
    }
}

pub fn emit_progress(progress_cb: Option<&ProgressCallbackFn>, progress: ProgressEvent) {
    if let Some(cb) = progress_cb {
        cb(progress);
    }
}

pub fn classify_load_state<T>(
    state: &T,
    is_ready: impl FnOnce(&T) -> bool,
    failed_error: impl FnOnce(&T) -> Option<String>,
    is_loading: impl FnOnce(&T) -> bool,
) -> LoadStateCheck {
    if is_ready(state) {
        LoadStateCheck::Ready
    } else if let Some(error) = failed_error(state) {
        LoadStateCheck::Failed(error)
    } else if is_loading(state) {
        LoadStateCheck::Waiting
    } else {
        LoadStateCheck::Unloaded
    }
}

pub fn set_failed_and_notify<T>(
    state: &mut T,
    ready_notify: &Notify,
    error: String,
    set_failed: impl FnOnce(&mut T, String),
) -> LocalModelError {
    set_failed(state, error.clone());
    ready_notify.notify_waiters();
    LocalModelError::loading_message(error)
}
