#![cfg(feature = "testkit")]
//! Integration tests for `StreamMiddleware`.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use swink_agent::{
    AgentContext, AssistantMessageEvent, Cost, StopReason, StreamFn, StreamMiddleware,
    StreamOptions, Usage,
};

use common::{MockStreamFn, default_model};

fn empty_context() -> AgentContext {
    AgentContext {
        system_prompt: String::new(),
        messages: vec![],
        tools: vec![],
    }
}

fn text_events() -> Vec<AssistantMessageEvent> {
    common::text_only_events("hello world")
}

#[tokio::test]
async fn logging_middleware_receives_all_events() {
    let inner: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![text_events()]));
    let count = Arc::new(AtomicU32::new(0));
    let count_clone = count.clone();

    let mw = StreamMiddleware::with_logging(inner, move |_| {
        count_clone.fetch_add(1, Ordering::SeqCst);
    });

    let model = default_model();
    let ctx = empty_context();
    let opts = StreamOptions::default();
    let ct = CancellationToken::new();
    let stream = mw.stream(&model, &ctx, &opts, ct);
    let collected: Vec<_> = stream.collect().await;

    assert_eq!(collected.len(), 5);
    assert_eq!(count.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn map_middleware_transforms_events() {
    let inner: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![text_events()]));
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

    let model = default_model();
    let ctx = empty_context();
    let opts = StreamOptions::default();
    let ct = CancellationToken::new();
    let stream = mw.stream(&model, &ctx, &opts, ct);
    let collected: Vec<_> = stream.collect().await;

    if let AssistantMessageEvent::TextDelta { delta, .. } = &collected[2] {
        assert_eq!(delta, "HELLO WORLD");
    } else {
        panic!("expected TextDelta at index 2");
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
    let inner: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![events]));

    let mw = StreamMiddleware::with_filter(inner, |event| {
        !matches!(
            event,
            AssistantMessageEvent::ThinkingStart { .. }
                | AssistantMessageEvent::ThinkingDelta { .. }
                | AssistantMessageEvent::ThinkingEnd { .. }
        )
    });

    let model = default_model();
    let ctx = empty_context();
    let opts = StreamOptions::default();
    let ct = CancellationToken::new();
    let stream = mw.stream(&model, &ctx, &opts, ct);
    let collected: Vec<_> = stream.collect().await;

    assert_eq!(collected.len(), 5);
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
    let inner: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![text_events()]));
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

    let model = default_model();
    let ctx = empty_context();
    let opts = StreamOptions::default();
    let ct = CancellationToken::new();
    let stream = mapped.stream(&model, &ctx, &opts, ct);
    let collected: Vec<_> = stream.collect().await;

    assert_eq!(count.load(Ordering::SeqCst), 5);
    if let AssistantMessageEvent::TextDelta { delta, .. } = &collected[2] {
        assert_eq!(delta, "[hello world]");
    } else {
        panic!("expected TextDelta at index 2");
    }
}
