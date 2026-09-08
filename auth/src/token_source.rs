use std::fmt;
use std::future::Future;
use std::sync::{PoisonError, RwLock};
use std::time::{Duration, Instant};

use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct ExpiringValue<T> {
    value: T,
    expires_at: Instant,
}

impl<T> fmt::Debug for ExpiringValue<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExpiringValue")
            .field("value", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
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

type RefreshFuture<T, E> = Shared<BoxFuture<'static, Result<ExpiringValue<T>, E>>>;

struct InFlightRefresh<T, E> {
    generation: u64,
    future: RefreshFuture<T, E>,
}

struct InFlightState<T, E> {
    current: Option<InFlightRefresh<T, E>>,
    next_generation: u64,
}

impl<T, E> InFlightState<T, E> {
    fn new() -> Self {
        Self {
            current: None,
            next_generation: 0,
        }
    }

    fn get_or_start_with(
        &mut self,
        start: impl FnOnce() -> RefreshFuture<T, E>,
    ) -> (u64, RefreshFuture<T, E>) {
        if let Some(current) = self.current.as_ref() {
            return (current.generation, current.future.clone());
        }

        let generation = self.next_generation;
        self.next_generation = self
            .next_generation
            .checked_add(1)
            .expect("single-flight refresh generation overflowed");
        let future = start();
        self.current = Some(InFlightRefresh {
            generation,
            future: future.clone(),
        });

        (generation, future)
    }

    fn clear_generation(&mut self, generation: u64) {
        if self
            .current
            .as_ref()
            .is_some_and(|current| current.generation == generation)
        {
            self.current = None;
        }
    }
}

pub struct SingleFlightTokenSource<T, E = String> {
    cached: RwLock<Option<ExpiringValue<T>>>,
    in_flight: Mutex<InFlightState<T, E>>,
    refresh_margin: Duration,
}

impl<T, E> SingleFlightTokenSource<T, E>
where
    T: Clone + Send + Sync + 'static,
    E: Clone + Send + Sync + 'static,
{
    #[must_use]
    pub fn new(refresh_margin: Duration) -> Self {
        Self {
            cached: RwLock::new(None),
            in_flight: Mutex::new(InFlightState::new()),
            refresh_margin,
        }
    }

    pub async fn get_or_refresh<F, Fut>(&self, refresh: F) -> Result<T, E>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<ExpiringValue<T>, E>> + Send + 'static,
    {
        {
            let cached = self.cached.read().unwrap_or_else(PoisonError::into_inner);
            if let Some(cached) = cached.as_ref()
                && Instant::now() + self.refresh_margin < cached.expires_at()
            {
                return Ok(cached.value().clone());
            }
        }

        let (generation, shared_future) = {
            let mut in_flight = self.in_flight.lock().await;
            in_flight.get_or_start_with(|| refresh().boxed().shared())
        };

        let refreshed = shared_future.await;
        if let Ok(value) = &refreshed {
            *self.cached.write().unwrap_or_else(PoisonError::into_inner) = Some(value.clone());
        }

        self.in_flight.lock().await.clear_generation(generation);
        refreshed.map(|value| value.value().clone())
    }

    /// Drop any cached value so the next lookup must rely on a fresh external
    /// snapshot or a new refresh. This is useful when the caller's source of
    /// truth lives outside this helper.
    pub fn clear_cached(&self) {
        *self.cached.write().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

impl<T, E> std::fmt::Debug for SingleFlightTokenSource<T, E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let has_cached = self
            .cached
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some();
        f.debug_struct("SingleFlightTokenSource")
            .field("has_cached", &has_cached)
            .field("refresh_margin", &self.refresh_margin)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[path = "token_source_tests.rs"]
mod tests;
