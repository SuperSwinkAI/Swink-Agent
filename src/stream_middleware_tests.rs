//! Tests for `stream_middleware`.
#![cfg(test)]

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
    let logged: Arc<dyn StreamFn> = Arc::new(StreamMiddleware::with_logging(inner, move |_| {
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
