//! Tests for `testing`.
#![cfg(test)]

use super::*;

fn verdict(pass: bool) -> JudgeVerdict {
    JudgeVerdict {
        score: if pass { 1.0 } else { 0.0 },
        pass,
        reason: None,
        label: None,
        cost: None,
    }
}

#[tokio::test]
async fn with_verdicts_replays_in_order() {
    let judge = MockJudge::with_verdicts(vec![verdict(true), verdict(false)]);
    let v1 = judge.judge("a").await.unwrap();
    assert!(v1.pass);
    let v2 = judge.judge("b").await.unwrap();
    assert!(!v2.pass);
}

#[tokio::test]
async fn with_verdicts_tail_errors_when_exhausted() {
    let judge = MockJudge::with_verdicts(vec![verdict(true)]);
    let _ = judge.judge("a").await.unwrap();
    let err = judge.judge("b").await.unwrap_err();
    match err {
        JudgeError::Other(msg) => assert!(msg.contains("exhausted")),
        other => panic!("expected Other, got {other:?}"),
    }
}

#[tokio::test]
async fn always_err_returns_configured_variant() {
    let judge = MockJudge::always_err(JudgeError::Timeout);
    for _ in 0..3 {
        match judge.judge("x").await {
            Err(JudgeError::Timeout) => {}
            other => panic!("expected Timeout, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn always_pass_fail_return_canned_verdicts() {
    let pass = MockJudge::always_pass();
    let p = pass.judge("x").await.unwrap();
    assert!(p.pass);
    let fail = MockJudge::always_fail();
    let f = fail.judge("x").await.unwrap();
    assert!(!f.pass);
}

#[tokio::test]
async fn mixed_sequence_preserves_order() {
    let judge = MockJudge::mixed_sequence(vec![
        Ok(verdict(true)),
        Err(JudgeError::MalformedResponse("bad".into())),
        Ok(verdict(false)),
    ]);
    assert!(judge.judge("a").await.unwrap().pass);
    match judge.judge("b").await.unwrap_err() {
        JudgeError::MalformedResponse(m) => assert_eq!(m, "bad"),
        other => panic!("expected MalformedResponse, got {other:?}"),
    }
    assert!(!judge.judge("c").await.unwrap().pass);
}

#[tokio::test]
async fn call_count_tracks_invocations() {
    let judge = MockJudge::always_pass();
    assert_eq!(judge.call_count(), 0);
    let _ = judge.judge("a").await;
    let _ = judge.judge("b").await;
    assert_eq!(judge.call_count(), 2);
}

#[tokio::test]
async fn dyn_dispatch_compiles() {
    use std::sync::Arc;
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let _ = judge.judge("prompt").await.unwrap();
}
