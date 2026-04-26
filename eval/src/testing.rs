//! Test doubles and helpers for the evaluation framework.
//!
//! This module is **always public, with no feature gate** — per the 2026-03-25
//! QA audit (`project_qa_audit.md`): test helpers live here so downstream crates
//! can consume them without enabling a test-only feature.
//!
//! Currently exposes [`MockJudge`], a canned [`JudgeClient`] implementation for
//! exercising semantic evaluators and error paths without a real LLM provider.

#![forbid(unsafe_code)]

use std::sync::Mutex;
use std::time::Duration;

use crate::judge::{JudgeClient, JudgeError, JudgeFuture, JudgeVerdict};

/// Canned `JudgeClient` for tests.
///
/// `MockJudge` replays a pre-configured sequence of judge outcomes, one per
/// call to [`JudgeClient::judge`]. When the sequence is exhausted the judge
/// returns the configured tail outcome (defaults to a [`JudgeError::Other`]
/// with an explanatory message so tests fail loudly rather than silently).
///
/// Call-count tracking is available via [`MockJudge::call_count`] so tests can
/// assert how many times the judge was invoked.
///
/// ```rust,ignore
/// use std::sync::Arc;
/// use swink_agent_eval::{JudgeClient, JudgeVerdict, MockJudge};
///
/// let judge = Arc::new(MockJudge::with_verdicts(vec![JudgeVerdict {
///     score: 1.0,
///     pass: true,
///     reason: Some("looks good".into()),
///     label: Some("equivalent".into()),
/// }]));
/// ```
pub struct MockJudge {
    inner: Mutex<MockInner>,
}

struct MockInner {
    /// Queued outcomes consumed in FIFO order.
    outcomes: Vec<MockOutcome>,
    /// Outcome to return once the queue is exhausted.
    tail: MockOutcome,
    /// Number of times `judge()` has been invoked.
    calls: usize,
}

enum MockOutcome {
    Verdict(JudgeVerdict),
    Error(JudgeError),
}

impl MockOutcome {
    fn clone_boxed(&self) -> Self {
        match self {
            Self::Verdict(v) => Self::Verdict(v.clone()),
            Self::Error(e) => Self::Error(clone_judge_error(e)),
        }
    }
}

fn clone_judge_error(err: &JudgeError) -> JudgeError {
    match err {
        JudgeError::Transport(s) => JudgeError::Transport(s.clone()),
        JudgeError::Timeout => JudgeError::Timeout,
        JudgeError::MalformedResponse(s) => JudgeError::MalformedResponse(s.clone()),
        JudgeError::Other(s) => JudgeError::Other(s.clone()),
    }
}

impl MockJudge {
    /// Build a mock judge that returns each verdict in order, then fails loudly
    /// with [`JudgeError::Other`] once exhausted.
    #[must_use]
    pub fn with_verdicts(verdicts: Vec<JudgeVerdict>) -> Self {
        let outcomes = verdicts.into_iter().map(MockOutcome::Verdict).collect();
        Self::new(
            outcomes,
            MockOutcome::Error(JudgeError::Other(
                "MockJudge outcome queue exhausted".into(),
            )),
        )
    }

    /// Build a mock judge that always returns the given [`JudgeError`].
    ///
    /// Useful for error-path tests (transport, malformed, timeout).
    #[must_use]
    pub const fn always_err(err: JudgeError) -> Self {
        Self::new(Vec::new(), MockOutcome::Error(err))
    }

    /// Build a mock judge that returns a single passing verdict every call.
    #[must_use]
    pub fn always_pass() -> Self {
        Self::new(
            Vec::new(),
            MockOutcome::Verdict(JudgeVerdict {
                score: 1.0,
                pass: true,
                reason: Some("mock pass".into()),
                label: None,
            }),
        )
    }

    /// Build a mock judge that returns a single failing verdict every call.
    #[must_use]
    pub fn always_fail() -> Self {
        Self::new(
            Vec::new(),
            MockOutcome::Verdict(JudgeVerdict {
                score: 0.0,
                pass: false,
                reason: Some("mock fail".into()),
                label: None,
            }),
        )
    }

    /// Build a mock judge from a mixed sequence of verdicts and errors. The
    /// sequence is consumed FIFO; once exhausted the `tail` outcome is
    /// returned on every subsequent call.
    #[must_use]
    pub fn mixed_sequence(sequence: Vec<Result<JudgeVerdict, JudgeError>>) -> Self {
        let outcomes = sequence
            .into_iter()
            .map(|r| match r {
                Ok(v) => MockOutcome::Verdict(v),
                Err(e) => MockOutcome::Error(e),
            })
            .collect();
        Self::new(
            outcomes,
            MockOutcome::Error(JudgeError::Other(
                "MockJudge outcome queue exhausted".into(),
            )),
        )
    }

    const fn new(outcomes: Vec<MockOutcome>, tail: MockOutcome) -> Self {
        Self {
            inner: Mutex::new(MockInner {
                outcomes,
                tail,
                calls: 0,
            }),
        }
    }

    /// Returns how many times `judge()` has been invoked on this mock.
    #[must_use]
    pub fn call_count(&self) -> usize {
        self.inner.lock().map(|g| g.calls).unwrap_or_default()
    }
}

impl JudgeClient for MockJudge {
    fn judge<'a>(&'a self, _prompt: &'a str) -> JudgeFuture<'a> {
        Box::pin(async move {
            let outcome = {
                let mut guard = self.inner.lock().expect("MockJudge mutex poisoned");
                guard.calls += 1;
                if guard.outcomes.is_empty() {
                    guard.tail.clone_boxed()
                } else {
                    guard.outcomes.remove(0)
                }
            };
            match outcome {
                MockOutcome::Verdict(v) => Ok(v),
                MockOutcome::Error(e) => Err(e),
            }
        })
    }
}

/// A `JudgeClient` test double that sleeps before returning a passing verdict.
///
/// Intended for exercising the evaluator-side outer `tokio::time::timeout`
/// (FR-010 / FR-014). Pair with
/// [`SemanticToolSelectionEvaluator::with_timeout`](crate::SemanticToolSelectionEvaluator::with_timeout)
/// or the equivalent on the tool-parameter evaluator to drive the outer
/// deadline path.
///
/// ```rust,ignore
/// use std::{sync::Arc, time::Duration};
/// use swink_agent_eval::{SemanticToolSelectionEvaluator, testing::SlowMockJudge};
///
/// let judge = Arc::new(SlowMockJudge::new(Duration::from_secs(10)));
/// let eval = SemanticToolSelectionEvaluator::new(judge)
///     .with_timeout(Duration::from_millis(50));
/// // `eval.evaluate(...)` will hit the outer timeout without hanging.
/// ```
pub struct SlowMockJudge {
    sleep: Duration,
}

impl SlowMockJudge {
    /// Build a slow judge that sleeps for the given duration before
    /// returning a single passing verdict.
    #[must_use]
    pub const fn new(sleep: Duration) -> Self {
        Self { sleep }
    }
}

impl JudgeClient for SlowMockJudge {
    fn judge<'a>(&'a self, _prompt: &'a str) -> JudgeFuture<'a> {
        Box::pin(async move {
            tokio::time::sleep(self.sleep).await;
            Ok(JudgeVerdict {
                score: 1.0,
                pass: true,
                reason: Some("slow pass".into()),
                label: None,
            })
        })
    }
}

/// A `JudgeClient` test double whose [`JudgeClient::judge`] implementation
/// always panics.
///
/// Exists to exercise the registry-level `catch_unwind` path (FR-014 / SC-008)
/// for the cross-cutting panic-isolation integration test in
/// `eval/tests/registry_panic_isolation.rs`. Pair with
/// [`crate::EvaluatorRegistry::with_defaults_and_judge`] to verify that a
/// panicking judge degrades every semantic evaluator to `Score::fail()` without
/// propagating the panic out of the runner.
///
/// ```rust,ignore
/// use std::sync::Arc;
/// use swink_agent_eval::{JudgeClient, PanickingMockJudge};
///
/// let judge: Arc<dyn JudgeClient> = Arc::new(PanickingMockJudge::new());
/// // `judge.judge("any prompt").await` panics with "judge panic".
/// ```
pub struct PanickingMockJudge {
    message: &'static str,
}

impl PanickingMockJudge {
    /// Build a panicking judge that panics with the default "judge panic"
    /// message on every call.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            message: "judge panic",
        }
    }

    /// Build a panicking judge that panics with a custom static message.
    #[must_use]
    pub const fn with_message(message: &'static str) -> Self {
        Self { message }
    }
}

impl Default for PanickingMockJudge {
    fn default() -> Self {
        Self::new()
    }
}

impl JudgeClient for PanickingMockJudge {
    fn judge<'a>(&'a self, _prompt: &'a str) -> JudgeFuture<'a> {
        Box::pin(async move { panic!("{}", self.message) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdict(pass: bool) -> JudgeVerdict {
        JudgeVerdict {
            score: if pass { 1.0 } else { 0.0 },
            pass,
            reason: None,
            label: None,
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
}
