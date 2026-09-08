//! Tests for `client`.
#![cfg(test)]

use super::*;
use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;

struct CountingJudge {
    calls: AtomicUsize,
}

impl JudgeClient for CountingJudge {
    fn judge<'a>(&'a self, _prompt: &'a str) -> swink_agent_eval::JudgeFuture<'a> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(JudgeVerdict::new(1.0, true))
        })
    }
}

#[tokio::test]
async fn batched_dispatch_preserves_order_and_chunks() {
    let inner = Arc::new(CountingJudge {
        calls: AtomicUsize::new(0),
    });
    let batched = BatchedJudgeClient::new(inner.clone(), 2).expect("valid batch size");
    let prompts = vec![
        "a".to_string(),
        "b".to_string(),
        "c".to_string(),
        "d".to_string(),
        "e".to_string(),
    ];
    let results = batched.judge_batch(&prompts).await;
    assert_eq!(results.len(), prompts.len());
    assert_eq!(inner.calls.load(Ordering::SeqCst), prompts.len());
    for r in results {
        assert!(r.is_ok());
    }
}

#[test]
fn block_on_judge_owns_runtime_without_ambient_tokio() {
    let result = block_on_judge(async { Ok(JudgeVerdict::new(0.7, true).with_reason("ok")) })
        .expect("blocking helper should build its own runtime");

    assert!(result.pass);
    assert!((result.score - 0.7).abs() < f64::EPSILON);
}

#[test]
fn block_on_judge_owns_runtime_inside_current_thread_runtime() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime");

    let result = runtime.block_on(async {
        block_on_judge(async { Ok(JudgeVerdict::new(1.0, true)) })
            .expect("current-thread runtime should delegate to an owned runtime")
    });

    assert!(result.pass);
    assert!((result.score - 1.0).abs() < f64::EPSILON);
}

#[test]
fn batch_size_zero_rejected() {
    let inner: Arc<dyn JudgeClient> = Arc::new(CountingJudge {
        calls: AtomicUsize::new(0),
    });
    let err = BatchedJudgeClient::new(inner, 0).expect_err("must reject zero");
    match err {
        JudgeError::Other(msg) => assert!(msg.contains("batch_size")),
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn batch_size_above_cap_rejected() {
    let inner: Arc<dyn JudgeClient> = Arc::new(CountingJudge {
        calls: AtomicUsize::new(0),
    });
    let err = BatchedJudgeClient::new(inner, MAX_BATCH_SIZE + 1).expect_err("must reject");
    match err {
        JudgeError::Other(msg) => assert!(msg.contains("batch_size")),
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn retry_classifier_only_retries_transport() {
    assert!(is_retryable(&JudgeError::Transport("429".into())));
    assert!(!is_retryable(&JudgeError::Timeout));
    assert!(!is_retryable(&JudgeError::MalformedResponse("x".into())));
    assert!(!is_retryable(&JudgeError::Other("x".into())));
}

#[tokio::test]
async fn retry_surfaces_cancellation_as_other() {
    let policy = fast_test_policy();
    let cancel = CancellationToken::new();
    cancel.cancel();

    let result = retry_with_cancel(&policy, &cancel, is_retryable, || async {
        Ok(JudgeVerdict::new(1.0, true))
    })
    .await;

    match result {
        Err(JudgeError::Other(msg)) => assert!(msg.contains("cancel")),
        other => panic!("expected cancellation, got {other:?}"),
    }
}

#[tokio::test]
async fn retry_cancels_in_flight_attempt() {
    let policy = fast_test_policy();
    let cancel = CancellationToken::new();
    let attempts = StdArc::new(AtomicUsize::new(0));
    let started = StdArc::new(Notify::new());

    let task = {
        let cancel = cancel.clone();
        let attempts = StdArc::clone(&attempts);
        let started = StdArc::clone(&started);
        tokio::spawn(async move {
            retry_with_cancel(&policy, &cancel, is_retryable, || {
                attempts.fetch_add(1, Ordering::SeqCst);
                started.notify_one();
                async { std::future::pending::<Result<JudgeVerdict, JudgeError>>().await }
            })
            .await
        })
    };

    started.notified().await;
    cancel.cancel();

    match task.await.expect("retry task should finish") {
        Err(JudgeError::Other(msg)) => assert!(msg.contains("cancel")),
        other => panic!("expected in-flight cancellation, got {other:?}"),
    }
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn retry_bails_on_terminal_error() {
    let policy = fast_test_policy();
    let cancel = CancellationToken::new();
    let attempts = AtomicUsize::new(0);

    let result = retry_with_cancel(&policy, &cancel, is_retryable, || {
        attempts.fetch_add(1, Ordering::SeqCst);
        async { Err::<JudgeVerdict, JudgeError>(JudgeError::MalformedResponse("x".into())) }
    })
    .await;

    assert!(matches!(result, Err(JudgeError::MalformedResponse(_))));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[test]
fn parse_verdict_plain_json() {
    let verdict = parse_verdict_text(r#"{"score": 0.8, "pass": true, "reason": "ok"}"#)
        .expect("verdict parses");
    assert!((verdict.score - 0.8).abs() < f64::EPSILON);
    assert!(verdict.pass);
    assert_eq!(verdict.reason.as_deref(), Some("ok"));
}

#[test]
fn parse_verdict_fenced_json() {
    let verdict =
        parse_verdict_text("```json\n{\"score\": 0.5, \"pass\": false}\n```").expect("parse");
    assert!(!verdict.pass);
    assert!((verdict.score - 0.5).abs() < f64::EPSILON);
}

#[test]
fn parse_verdict_clamps_out_of_range_score() {
    let v = parse_verdict_text(r#"{"score": 1.8, "pass": true}"#).expect("parse");
    assert!((v.score - 1.0).abs() < f64::EPSILON);
}

#[test]
fn parse_verdict_missing_pass_is_malformed() {
    let err = parse_verdict_text(r#"{"score": 0.5}"#).expect_err("must fail");
    assert!(matches!(err, JudgeError::MalformedResponse(_)));
}

#[test]
fn parse_verdict_non_json_is_malformed() {
    let err = parse_verdict_text("not json at all").expect_err("must fail");
    assert!(matches!(err, JudgeError::MalformedResponse(_)));
}

#[tokio::test]
async fn retry_retries_transport_up_to_cap() {
    let policy = fast_test_policy();
    let cancel = CancellationToken::new();
    let attempts = AtomicUsize::new(0);

    let result = retry_with_cancel(&policy, &cancel, is_retryable, || {
        attempts.fetch_add(1, Ordering::SeqCst);
        async { Err::<JudgeVerdict, JudgeError>(JudgeError::Transport("429".into())) }
    })
    .await;

    assert!(matches!(result, Err(JudgeError::Transport(_))));
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        policy.max_attempts as usize
    );
}
