use std::future::Future;
use std::sync::Arc;

use tokio::sync::Notify;

use crate::error::LocalModelError;
use crate::progress::{ProgressCallbackFn, ProgressEvent};

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
