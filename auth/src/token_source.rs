use std::future::Future;
use std::sync::{PoisonError, RwLock};
use std::time::{Duration, Instant};

use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use tokio::sync::Mutex;

#[derive(Clone, Debug)]
pub struct ExpiringValue<T> {
    value: T,
    expires_at: Instant,
}

impl<T> ExpiringValue<T> {
    #[must_use]
    pub fn new(value: T, expires_at: Instant) -> Self {
        Self { value, expires_at }
    }

    #[must_use]
    pub fn value(&self) -> &T {
        &self.value
    }

    #[must_use]
    pub fn expires_at(&self) -> Instant {
        self.expires_at
    }
}

type RefreshFuture<T> = Shared<BoxFuture<'static, Result<ExpiringValue<T>, String>>>;

pub struct SingleFlightTokenSource<T> {
    cached: RwLock<Option<ExpiringValue<T>>>,
    in_flight: Mutex<Option<RefreshFuture<T>>>,
    refresh_margin: Duration,
}

impl<T> SingleFlightTokenSource<T>
where
    T: Clone + Send + Sync + 'static,
{
    #[must_use]
    pub fn new(refresh_margin: Duration) -> Self {
        Self {
            cached: RwLock::new(None),
            in_flight: Mutex::new(None),
            refresh_margin,
        }
    }

    pub async fn get_or_refresh<F, Fut>(&self, refresh: F) -> Result<T, String>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<ExpiringValue<T>, String>> + Send + 'static,
    {
        {
            let cached = self.cached.read().unwrap_or_else(PoisonError::into_inner);
            if let Some(cached) = cached.as_ref()
                && Instant::now() + self.refresh_margin < cached.expires_at()
            {
                return Ok(cached.value().clone());
            }
        }

        let shared_future = {
            let mut in_flight = self.in_flight.lock().await;
            if let Some(existing) = in_flight.as_ref() {
                existing.clone()
            } else {
                let shared = refresh().boxed().shared();
                *in_flight = Some(shared.clone());
                shared
            }
        };

        let refreshed = shared_future.await;
        if let Ok(value) = &refreshed {
            *self.cached.write().unwrap_or_else(PoisonError::into_inner) = Some(value.clone());
        }

        self.in_flight.lock().await.take();
        refreshed.map(|value| value.value().clone())
    }
}

impl<T> std::fmt::Debug for SingleFlightTokenSource<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let has_cached = self
            .cached
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some();
        f.debug_struct("SingleFlightTokenSource")
            .field("has_cached", &has_cached)
            .field("refresh_margin", &self.refresh_margin)
            .finish()
    }
}
