//! Shared retry, cancellation, and batching helpers used by every judge
//! client in this crate.
//!
//! The building blocks are deliberately small:
//!
//! * [`build_retry`] wraps [`RetryPolicy`] into a `backon::ExponentialBuilder`
//!   pinned to the policy values from research.md §R-002 (6 attempts,
//!   4-minute max delay, jitter on).
//! * [`retry_with_cancel`] runs an async factory under that builder while
//!   racing each attempt against a [`CancellationToken`], so cancellation
//!   surfaces as [`JudgeError::Other`] rather than waiting out the backoff
//!   schedule.
//! * [`BatchedJudgeClient`] wraps a [`JudgeClient`] and exposes a
//!   [`BatchedJudgeClient::judge_batch`] convenience that dispatches up to
//!   `batch_size` prompts. Providers that do not support native batching fall
//!   through to sequential dispatch here (FR-005).
//! * [`BlockingExt`] is a small adapter so provider-specific `Blocking*`
//!   wrappers can reuse a single `block_on` helper.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use backon::{ExponentialBuilder, Retryable};
use tokio_util::sync::CancellationToken;

use swink_agent_eval::judge::{JudgeClient, JudgeError, JudgeVerdict, MAX_BATCH_SIZE, RetryPolicy};

/// Convert a workspace [`RetryPolicy`] into a `backon::ExponentialBuilder`
/// pinned to the research.md §R-002 schedule (jitter on by default).
///
/// `RetryPolicy::max_attempts` is the **total** attempts allowed (initial
/// call plus retries), matching FR-004 ("up to 6 attempts"). Backon's
/// `with_max_times` is the retry count, so this helper subtracts one from
/// `max_attempts` with a floor of zero — i.e. `max_attempts = 1` means
/// "one shot, no retries".
#[must_use]
pub fn build_retry(policy: &RetryPolicy) -> ExponentialBuilder {
    let max_retries = policy.max_attempts.saturating_sub(1) as usize;
    // Floor the per-attempt minimum at the policy's `max_delay` so tight
    // test policies (`max_delay = 10ms`) don't inherit the backon default
    // of 1 s. In production, `max_delay = 4 min` dominates `min_delay =
    // 1 s` and the schedule is unaffected.
    let min_delay = std::cmp::min(Duration::from_secs(1), policy.max_delay);
    let mut builder = ExponentialBuilder::default()
        .with_max_times(max_retries)
        .with_min_delay(min_delay)
        .with_max_delay(policy.max_delay);
    if policy.jitter {
        builder = builder.with_jitter();
    }
    builder
}

/// Run a retryable async factory under [`build_retry`] while racing each
/// attempt against `cancel`.
///
/// * `should_retry` classifies [`JudgeError`] values. It must return `true`
///   only for transient errors (e.g. HTTP 429 / 5xx). Terminal errors are
///   bubbled through immediately so unit-test expectations stay tight.
/// * Cancellation wins the race: if the token fires mid-attempt, the future
///   resolves to [`JudgeError::Other`] with a cancellation tag and the
///   builder schedule is abandoned.
pub async fn retry_with_cancel<Fut, Factory, Retry>(
    policy: &RetryPolicy,
    cancel: &CancellationToken,
    should_retry: Retry,
    factory: Factory,
) -> Result<JudgeVerdict, JudgeError>
where
    Factory: FnMut() -> Fut,
    Fut: Future<Output = Result<JudgeVerdict, JudgeError>>,
    Retry: Fn(&JudgeError) -> bool,
{
    if cancel.is_cancelled() {
        return Err(JudgeError::Other("cancelled".to_string()));
    }

    let builder = build_retry(policy);
    // Wrap the Retry driver in an async block so `tokio::select!` can
    // race it against cancellation uniformly.
    let driver = async move { factory.retry(builder).when(should_retry).await };

    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(JudgeError::Other("cancelled".to_string())),
        res = driver => res,
    }
}

/// Classifier: return `true` for [`JudgeError`] variants that a judge client should retry.
///
/// Transport errors are assumed transient (the provider layer tags 429 / 5xx
/// as [`JudgeError::Transport`]); every other variant is terminal.
#[must_use]
pub fn is_retryable(err: &JudgeError) -> bool {
    matches!(err, JudgeError::Transport(_))
}

/// Wrapper around a [`JudgeClient`] providing a batched dispatch surface.
///
/// Call sites ask for `judge_batch(prompts)`; this helper enforces the
/// bounded batch size `[1, 128]` from FR-005 and falls back to sequential
/// dispatch for providers that do not ship a native batch endpoint. When a
/// provider later grows a native batch call it can reimplement this wrapper
/// without disturbing callers.
pub struct BatchedJudgeClient {
    inner: Arc<dyn JudgeClient>,
    batch_size: usize,
}

impl BatchedJudgeClient {
    /// Error value returned when `batch_size` falls outside the supported
    /// `[1, 128]` window.
    pub const INVALID_BATCH_MESSAGE: &'static str = "batch_size must be in 1..=128";

    /// Build a batching wrapper around `inner` with the supplied
    /// `batch_size`.
    ///
    /// Returns [`JudgeError::Other`] when `batch_size` is outside the
    /// supported window to match the rest of the judge-layer error model.
    pub fn new(inner: Arc<dyn JudgeClient>, batch_size: usize) -> Result<Self, JudgeError> {
        if !(1..=MAX_BATCH_SIZE).contains(&batch_size) {
            return Err(JudgeError::Other(format!(
                "{}, got {batch_size}",
                Self::INVALID_BATCH_MESSAGE
            )));
        }
        Ok(Self { inner, batch_size })
    }

    /// Bounded batch size, always in `[1, 128]`.
    #[must_use]
    pub const fn batch_size(&self) -> usize {
        self.batch_size
    }

    /// Borrow the wrapped judge client.
    #[must_use]
    pub fn inner(&self) -> &Arc<dyn JudgeClient> {
        &self.inner
    }

    /// Dispatch `prompts` in chunks of at most `batch_size`.
    ///
    /// Non-native-batch providers fall through to sequential dispatch here;
    /// the wrapper preserves input order and returns one `Result<_, _>` per
    /// prompt (never short-circuits on the first error) so callers can
    /// decide per-case remediation.
    pub async fn judge_batch(&self, prompts: &[String]) -> Vec<Result<JudgeVerdict, JudgeError>> {
        let mut out = Vec::with_capacity(prompts.len());
        for chunk in prompts.chunks(self.batch_size) {
            for prompt in chunk {
                out.push(self.inner.judge(prompt).await);
            }
        }
        out
    }
}

impl std::fmt::Debug for BatchedJudgeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BatchedJudgeClient")
            .field("batch_size", &self.batch_size)
            .finish_non_exhaustive()
    }
}

/// Extension trait for provider-specific blocking wrappers.
pub trait BlockingExt: Future + Send + 'static
where
    Self::Output: Send + 'static,
{
    /// Block on the future using the current Tokio runtime handle.
    fn block_on(self) -> Self::Output
    where
        Self: Sized,
    {
        tokio::runtime::Handle::current().block_on(self)
    }
}

impl<F> BlockingExt for F
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
}

/// Small policy used by tests and callers that want a tight retry schedule.
#[must_use]
pub const fn fast_test_policy() -> RetryPolicy {
    RetryPolicy::new(3, Duration::from_millis(50), false)
}

/// Parse a judge verdict out of an arbitrary text blob.
///
/// Supports bare JSON objects or the same object wrapped in a fenced code
/// block. Each judge client extracts the provider's text content and funnels
/// it through this helper so the verdict schema stays identical across
/// providers.
pub fn parse_verdict_text(text: &str) -> Result<JudgeVerdict, JudgeError> {
    let cleaned = strip_code_fence(text.trim());
    let value: serde_json::Value = serde_json::from_str(cleaned)
        .map_err(|e| JudgeError::MalformedResponse(format!("verdict not valid JSON: {e}")))?;

    let obj = value
        .as_object()
        .ok_or_else(|| JudgeError::MalformedResponse("verdict must be a JSON object".into()))?;

    let score = obj
        .get("score")
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| JudgeError::MalformedResponse("verdict missing numeric `score`".into()))?;
    let pass = obj
        .get("pass")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| JudgeError::MalformedResponse("verdict missing boolean `pass`".into()))?;
    let reason = obj
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let label = obj
        .get("label")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);

    Ok(JudgeVerdict {
        score: score.clamp(0.0, 1.0),
        pass,
        reason,
        label,
    })
}

fn strip_code_fence(text: &str) -> &str {
    let trimmed = text.trim();
    if let Some(rest) = trimmed.strip_prefix("```json") {
        return rest.trim().trim_end_matches("```").trim();
    }
    if let Some(rest) = trimmed.strip_prefix("```") {
        return rest.trim().trim_end_matches("```").trim();
    }
    trimmed
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingJudge {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl JudgeClient for CountingJudge {
        async fn judge(&self, _prompt: &str) -> Result<JudgeVerdict, JudgeError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(JudgeVerdict {
                score: 1.0,
                pass: true,
                reason: None,
                label: None,
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
            Ok(JudgeVerdict {
                score: 1.0,
                pass: true,
                reason: None,
                label: None,
            })
        })
        .await;

        match result {
            Err(JudgeError::Other(msg)) => assert!(msg.contains("cancel")),
            other => panic!("expected cancellation, got {other:?}"),
        }
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
}
