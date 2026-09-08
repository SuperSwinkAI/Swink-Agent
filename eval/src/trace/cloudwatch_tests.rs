//! Tests for `cloudwatch`.
#![cfg(test)]

use super::*;

struct StaticFetcher(Vec<serde_json::Value>);

impl CloudWatchLogsFetcher for StaticFetcher {
    fn fetch_events<'a>(&'a self, _session_id: &'a str) -> CloudWatchFetchFuture<'a> {
        Box::pin(async move { Ok(self.0.clone()) })
    }
}

#[tokio::test]
async fn session_not_found_when_fetcher_returns_empty() {
    let provider = CloudWatchTraceProvider::new(Arc::new(StaticFetcher(vec![])) as Arc<_>);
    let err = provider.fetch_session("sid").await.expect_err("empty");
    assert!(matches!(err, TraceProviderError::SessionNotFound { .. }));
}

#[tokio::test]
async fn session_in_progress_when_any_span_has_no_end_time() {
    let events = vec![
        serde_json::json!({
            "name": "root",
            "span_id": "s1",
            "start_time": "2026-04-23T10:00:00Z",
            "end_time": "2026-04-23T10:00:01Z",
        }),
        serde_json::json!({
            "name": "open",
            "span_id": "s2",
            "start_time": "2026-04-23T10:00:02Z",
        }),
    ];
    let provider = CloudWatchTraceProvider::new(Arc::new(StaticFetcher(events)) as Arc<_>);
    let err = provider.fetch_session("sid").await.expect_err("open span");
    match err {
        TraceProviderError::SessionInProgress { open_spans, .. } => {
            assert_eq!(open_spans, 1);
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[tokio::test]
async fn happy_path_emits_spans_with_session_attribute() {
    let events = vec![serde_json::json!({
        "name": "root",
        "span_id": "s1",
        "start_time": "2026-04-23T10:00:00Z",
        "end_time": "2026-04-23T10:00:01Z",
        "kind": "client",
        "attributes": {
            "model": "gpt-4",
            "token.count": 42,
        },
    })];
    let provider = CloudWatchTraceProvider::new(Arc::new(StaticFetcher(events)) as Arc<_>);
    let raw = provider.fetch_session("sid").await.expect("ok");
    match raw {
        RawSession::OtelSpans { session_id, spans } => {
            assert_eq!(session_id, "sid");
            assert_eq!(spans.len(), 1);
            let attrs: Vec<&str> = spans[0]
                .attributes
                .iter()
                .map(|kv| kv.key.as_str())
                .collect();
            assert!(attrs.contains(&"session.id"));
            assert!(attrs.contains(&"model"));
            assert!(attrs.contains(&"token.count"));
        }
    }
}
