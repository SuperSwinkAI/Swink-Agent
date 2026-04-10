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
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use futures::StreamExt;

    use super::*;
    use crate::stream::AssistantMessageEvent;
    use crate::types::{AgentContext, Cost, ModelSpec, StopReason, Usage};

    /// Minimal `StreamFn` for testing.
    struct TestStreamFn {
        events: std::sync::Mutex<Vec<AssistantMessageEvent>>,
    }

    impl TestStreamFn {
        fn new(events: Vec<AssistantMessageEvent>) -> Self {
            Self {
                events: std::sync::Mutex::new(events),
            }
        }
    }

    impl StreamFn for TestStreamFn {
        fn stream<'a>(
            &'a self,
            _model: &'a ModelSpec,
            _context: &'a AgentContext,
            _options: &'a StreamOptions,
            _ct: CancellationToken,
        ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
            let events = self.events.lock().unwrap().clone();
            Box::pin(futures::stream::iter(events))
        }
    }

    fn test_events() -> Vec<AssistantMessageEvent> {
        vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::TextStart { content_index: 0 },
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "hello".into(),
            },
            AssistantMessageEvent::TextEnd { content_index: 0 },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::Stop,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ]
    }

    fn test_model() -> ModelSpec {
        ModelSpec::new("test", "test-model")
    }

    fn test_context() -> AgentContext {
        AgentContext {
            system_prompt: String::new(),
            messages: vec![],
            tools: vec![],
        }
    }

    #[tokio::test]
    async fn logging_middleware_receives_all_events() {
        let inner = Arc::new(TestStreamFn::new(test_events()));
        let count = Arc::new(AtomicU32::new(0));
        let count_clone = count.clone();

        let mw = StreamMiddleware::with_logging(inner, move |_event| {
            count_clone.fetch_add(1, Ordering::SeqCst);
        });

        let model = test_model();
        let ctx = test_context();
        let opts = StreamOptions::default();
        let ct = CancellationToken::new();
        let stream = mw.stream(&model, &ctx, &opts, ct);
        let collected: Vec<_> = stream.collect().await;

        assert_eq!(collected.len(), 5);
        assert_eq!(count.load(Ordering::SeqCst), 5);
    }

    #[tokio::test]
    async fn map_middleware_transforms_events() {
        let inner = Arc::new(TestStreamFn::new(test_events()));
        let mw = StreamMiddleware::with_map(inner, |event| match event {
            AssistantMessageEvent::TextDelta {
                content_index,
                delta,
            } => AssistantMessageEvent::TextDelta {
                content_index,
                delta: delta.to_uppercase(),
            },
            other => other,
        });

        let model = test_model();
        let ctx = test_context();
        let opts = StreamOptions::default();
        let ct = CancellationToken::new();
        let stream = mw.stream(&model, &ctx, &opts, ct);
        let collected: Vec<_> = stream.collect().await;

        let text_delta = &collected[2];
        if let AssistantMessageEvent::TextDelta { delta, .. } = text_delta {
            assert_eq!(delta, "HELLO");
        } else {
            panic!("expected TextDelta");
        }
    }

    #[tokio::test]
    async fn filter_middleware_drops_thinking_events() {
        let events = vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::ThinkingStart { content_index: 0 },
            AssistantMessageEvent::ThinkingDelta {
                content_index: 0,
                delta: "reasoning...".into(),
            },
            AssistantMessageEvent::ThinkingEnd {
                content_index: 0,
                signature: None,
            },
            AssistantMessageEvent::TextStart { content_index: 1 },
            AssistantMessageEvent::TextDelta {
                content_index: 1,
                delta: "result".into(),
            },
            AssistantMessageEvent::TextEnd { content_index: 1 },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::Stop,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ];
        let inner = Arc::new(TestStreamFn::new(events));
        let mw = StreamMiddleware::with_filter(inner, |event| {
            !matches!(
                event,
                AssistantMessageEvent::ThinkingStart { .. }
                    | AssistantMessageEvent::ThinkingDelta { .. }
                    | AssistantMessageEvent::ThinkingEnd { .. }
            )
        });

        let model = test_model();
        let ctx = test_context();
        let opts = StreamOptions::default();
        let ct = CancellationToken::new();
        let stream = mw.stream(&model, &ctx, &opts, ct);
        let collected: Vec<_> = stream.collect().await;

        // Start + TextStart + TextDelta + TextEnd + Done = 5
        assert_eq!(collected.len(), 5);
        // No thinking events
        for event in &collected {
            assert!(!matches!(
                event,
                AssistantMessageEvent::ThinkingStart { .. }
                    | AssistantMessageEvent::ThinkingDelta { .. }
                    | AssistantMessageEvent::ThinkingEnd { .. }
            ));
        }
    }

    #[tokio::test]
    async fn middleware_chains_compose() {
        let inner = Arc::new(TestStreamFn::new(test_events()));
        let count = Arc::new(AtomicU32::new(0));
        let count_clone = count.clone();

        // First layer: log
        let logged: Arc<dyn StreamFn> =
            Arc::new(StreamMiddleware::with_logging(inner, move |_| {
                count_clone.fetch_add(1, Ordering::SeqCst);
            }));

        // Second layer: map
        let mapped = StreamMiddleware::with_map(logged, |event| match event {
            AssistantMessageEvent::TextDelta {
                content_index,
                delta,
            } => AssistantMessageEvent::TextDelta {
                content_index,
                delta: format!("[{delta}]"),
            },
            other => other,
        });

        let model = test_model();
        let ctx = test_context();
        let opts = StreamOptions::default();
        let ct = CancellationToken::new();
        let stream = mapped.stream(&model, &ctx, &opts, ct);
        let collected: Vec<_> = stream.collect().await;

        // Logging saw all 5 events
        assert_eq!(count.load(Ordering::SeqCst), 5);
        // Map transformed the delta
        if let AssistantMessageEvent::TextDelta { delta, .. } = &collected[2] {
            assert_eq!(delta, "[hello]");
        } else {
            panic!("expected TextDelta");
        }
    }
}
