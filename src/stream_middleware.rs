//! Middleware wrapper for [`StreamFn`] that intercepts the output stream.
//!
//! Mirrors the [`ToolMiddleware`](crate::ToolMiddleware) pattern but for the
//! streaming boundary. Wraps an `Arc<dyn StreamFn>` and transforms the output
//! stream of [`AssistantMessageEvent`] values.
//!
//! # Example
//!
//! ```
//! use std::sync::Arc;
//! use swink_agent::{StreamMiddleware, AssistantMessageEvent};
//! # use swink_agent::StreamFn;
//! # fn example(stream_fn: Arc<dyn StreamFn>) {
//! let logged = StreamMiddleware::with_logging(stream_fn, |event| {
//!     println!("event: {event:?}");
//! });
//! # }
//! ```

use std::pin::Pin;
use std::sync::Arc;

use futures::stream::{Stream, StreamExt};
use tokio_util::sync::CancellationToken;

use crate::stream::{AssistantMessageEvent, StreamFn, StreamOptions};
use crate::types::{AgentContext, ModelSpec};

// ─── Type alias for the stream transformation closure ───────────────────────

type MapStreamFn = Arc<
    dyn for<'a> Fn(
            Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>>,
        ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>>
        + Send
        + Sync,
>;

// ─── StreamMiddleware ───────────────────────────────────────────────────────

/// Intercepts the output stream from a wrapped [`StreamFn`].
///
/// The inner `StreamFn` is called normally, then `map_stream` transforms
/// the resulting event stream before it reaches the consumer.
pub struct StreamMiddleware {
    inner: Arc<dyn StreamFn>,
    map_stream: MapStreamFn,
}

impl StreamMiddleware {
    /// Create a new middleware with a full stream transformation.
    ///
    /// The closure receives the inner stream and returns a transformed stream.
    pub fn new<F>(inner: Arc<dyn StreamFn>, f: F) -> Self
    where
        F: for<'a> Fn(
                Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>>,
            )
                -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>>
            + Send
            + Sync
            + 'static,
    {
        Self {
            inner,
            map_stream: Arc::new(f),
        }
    }

    /// Create a middleware that inspects each event via a logging callback.
    ///
    /// Events pass through unmodified; the callback is called for each event.
    pub fn with_logging<F>(inner: Arc<dyn StreamFn>, callback: F) -> Self
    where
        F: Fn(&AssistantMessageEvent) + Send + Sync + 'static,
    {
        let callback = Arc::new(callback);
        Self::new(inner, move |stream| {
            let cb = callback.clone();
            Box::pin(stream.inspect(move |event| cb(event)))
        })
    }

    /// Create a middleware that maps each event through a transformation.
    pub fn with_map<F>(inner: Arc<dyn StreamFn>, f: F) -> Self
    where
        F: Fn(AssistantMessageEvent) -> AssistantMessageEvent + Send + Sync + 'static,
    {
        let f = Arc::new(f);
        Self::new(inner, move |stream| {
            let f = f.clone();
            Box::pin(stream.map(move |event| f(event)))
        })
    }

    /// Create a middleware that filters events based on a predicate.
    ///
    /// Events for which the predicate returns `false` are dropped from the stream.
    pub fn with_filter<F>(inner: Arc<dyn StreamFn>, f: F) -> Self
    where
        F: Fn(&AssistantMessageEvent) -> bool + Send + Sync + 'static,
    {
        let f = Arc::new(f);
        Self::new(inner, move |stream| {
            let f = f.clone();
            Box::pin(stream.filter(move |event| {
                let keep = f(event);
                async move { keep }
            }))
        })
    }
}

impl StreamFn for StreamMiddleware {
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        let inner_stream = self
            .inner
            .stream(model, context, options, cancellation_token);
        (self.map_stream)(inner_stream)
    }
}

impl std::fmt::Debug for StreamMiddleware {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamMiddleware").finish_non_exhaustive()
    }
}

// ─── Compile-time Send + Sync assertion ─────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<StreamMiddleware>();
};

#[cfg(test)]
#[path = "stream_middleware_tests.rs"]
mod tests;
