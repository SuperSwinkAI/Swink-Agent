//! Tests for `mod`.
#![cfg(test)]

use super::*;
use crate::judge::{JudgeClient, JudgeRegistry};
use crate::prompt::{MinijinjaTemplate, PromptContext, PromptFamily};
use crate::types::{EvalCase, Invocation};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use swink_agent::{Cost, ModelSpec, StopReason, Usage};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

struct FixedJudge {
    score: f64,
    reason: Option<String>,
    last_prompt: Mutex<Option<String>>,
}

impl JudgeClient for FixedJudge {
    fn judge<'a>(&'a self, prompt: &'a str) -> crate::judge::JudgeFuture<'a> {
        Box::pin(async move {
            *self.last_prompt.lock().unwrap() = Some(prompt.to_string());
            Ok(JudgeVerdict {
                score: self.score,
                pass: (0.5..=1.0).contains(&self.score),
                reason: self.reason.clone(),
                label: None,
                cost: None,
            })
        })
    }
}

struct ScriptedJudge {
    outcomes: Mutex<VecDeque<Result<JudgeVerdict, JudgeError>>>,
    prompts: Mutex<Vec<String>>,
}

impl ScriptedJudge {
    fn new(outcomes: Vec<Result<JudgeVerdict, JudgeError>>) -> Self {
        Self {
            outcomes: Mutex::new(outcomes.into()),
            prompts: Mutex::new(Vec::new()),
        }
    }

    fn prompt_count(&self) -> usize {
        self.prompts.lock().unwrap().len()
    }
}

impl JudgeClient for ScriptedJudge {
    fn judge<'a>(&'a self, prompt: &'a str) -> crate::judge::JudgeFuture<'a> {
        Box::pin(async move {
            self.prompts.lock().unwrap().push(prompt.to_string());
            self.outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err(JudgeError::Other("script exhausted".to_string())))
        })
    }
}

#[derive(Default)]
struct PendingJudge {
    started: Notify,
    prompts: AtomicUsize,
}

impl PendingJudge {
    fn prompt_count(&self) -> usize {
        self.prompts.load(Ordering::SeqCst)
    }
}

impl JudgeClient for PendingJudge {
    fn judge<'a>(&'a self, _prompt: &'a str) -> crate::judge::JudgeFuture<'a> {
        Box::pin(async move {
            self.prompts.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            futures::future::pending().await
        })
    }
}

fn make_case() -> EvalCase {
    EvalCase {
        id: "case-1".into(),
        name: "Case One".into(),
        description: None,
        system_prompt: "answer".into(),
        user_messages: vec!["hi".into()],
        expected_trajectory: None,
        expected_response: None,
        expected_assertion: None,
        expected_interactions: None,
        few_shot_examples: vec![],
        budget: None,
        evaluators: vec![],
        metadata: serde_json::Value::Null,
        attachments: vec![],
        session_id: None,
        expected_environment_state: None,
        expected_tool_intent: None,
        semantic_tool_selection: false,
        state_capture: None,
    }
}

fn make_invocation() -> Invocation {
    Invocation {
        turns: vec![],
        total_usage: Usage::default(),
        total_cost: Cost::default(),
        total_duration: Duration::from_millis(1),
        final_response: Some("42".into()),
        stop_reason: StopReason::Stop,
        model: ModelSpec::new("test", "judge-target"),
    }
}

fn make_registry(score: f64) -> (Arc<JudgeRegistry>, Arc<FixedJudge>) {
    let judge = Arc::new(FixedJudge {
        score,
        reason: Some("ok".into()),
        last_prompt: Mutex::new(None),
    });
    let registry = JudgeRegistry::builder(judge.clone() as Arc<dyn JudgeClient>, "mock-model")
        .build()
        .expect("registry builds");
    (Arc::new(registry), judge)
}

fn make_retry_registry(judge: Arc<ScriptedJudge>, max_attempts: u32) -> Arc<JudgeRegistry> {
    Arc::new(
        JudgeRegistry::builder(judge as Arc<dyn JudgeClient>, "mock-model")
            .with_retry_policy(crate::judge::RetryPolicy::new(
                max_attempts,
                Duration::from_millis(1),
                false,
            ))
            .build()
            .expect("registry builds"),
    )
}

fn make_template() -> Arc<dyn JudgePromptTemplate> {
    Arc::new(
        MinijinjaTemplate::new(
            "mock_v0",
            PromptFamily::Quality,
            "Case={{ case.name }} Actual={{ invocation.final_response }}",
        )
        .expect("template compiles"),
    )
}

fn make_context(case: &EvalCase, invocation: &Invocation) -> PromptContext {
    PromptContext::new(Arc::new(case.clone()), Arc::new(invocation.clone()))
}

#[tokio::test]
async fn dispatch_records_prompt_version() {
    let (registry, _) = make_registry(0.8);
    let config = JudgeEvaluatorConfig::default_with(registry);
    let template = make_template();
    let case = make_case();
    let invocation = make_invocation();
    let ctx = make_context(&case, &invocation);

    let outcome = dispatch_judge(&config, template, &ctx).await.unwrap();

    assert!(
        outcome
            .details
            .entries()
            .iter()
            .any(|d| matches!(d, Detail::PromptVersion { version } if version == "mock_v0"))
    );
    assert!(
        !outcome
            .details
            .entries()
            .iter()
            .any(|d| matches!(d, Detail::ScoreClamped { .. }))
    );
    assert!((outcome.score.value - 0.8).abs() < f64::EPSILON);
}

#[tokio::test]
async fn dispatch_retries_transport_errors_with_registry_policy() {
    let judge = Arc::new(ScriptedJudge::new(vec![
        Err(JudgeError::Transport("temporary 503".to_string())),
        Err(JudgeError::Transport("temporary 429".to_string())),
        Ok(JudgeVerdict {
            score: 0.9,
            pass: true,
            reason: Some("recovered".to_string()),
            label: None,
            cost: None,
        }),
    ]));
    let registry = make_retry_registry(Arc::clone(&judge), 3);
    let config = JudgeEvaluatorConfig::default_with(registry);
    let template = make_template();
    let case = make_case();
    let invocation = make_invocation();
    let ctx = make_context(&case, &invocation);

    let outcome = dispatch_judge(&config, template, &ctx)
        .await
        .expect("transport failures should retry and recover");

    assert!((outcome.score.value - 0.9).abs() < f64::EPSILON);
    assert_eq!(judge.prompt_count(), 3);
}

#[tokio::test]
async fn dispatch_does_not_retry_terminal_judge_errors() {
    let judge = Arc::new(ScriptedJudge::new(vec![
        Err(JudgeError::MalformedResponse("bad json".to_string())),
        Ok(JudgeVerdict {
            score: 1.0,
            pass: true,
            reason: None,
            label: None,
            cost: None,
        }),
    ]));
    let registry = make_retry_registry(Arc::clone(&judge), 3);
    let config = JudgeEvaluatorConfig::default_with(registry);
    let template = make_template();
    let case = make_case();
    let invocation = make_invocation();
    let ctx = make_context(&case, &invocation);

    let err = dispatch_judge(&config, template, &ctx)
        .await
        .expect_err("malformed verdicts are terminal");

    assert!(matches!(
        err,
        DispatchError::Judge(JudgeError::MalformedResponse(_))
    ));
    assert_eq!(judge.prompt_count(), 1);
}

#[tokio::test]
async fn dispatch_cancels_in_flight_judge_future() {
    let judge = Arc::new(PendingJudge::default());
    let cancel = CancellationToken::new();
    let registry = Arc::new(
        JudgeRegistry::builder(Arc::clone(&judge) as Arc<dyn JudgeClient>, "mock-model")
            .with_cancellation(cancel.clone())
            .build()
            .expect("registry builds"),
    );
    let config = JudgeEvaluatorConfig::default_with(registry);
    let template = make_template();
    let case = make_case();
    let invocation = make_invocation();
    let ctx = make_context(&case, &invocation);

    let started = judge.started.notified();
    let dispatch = tokio::spawn(async move { dispatch_judge(&config, template, &ctx).await });
    started.await;
    cancel.cancel();

    let err = dispatch
        .await
        .expect("dispatch task should not panic")
        .expect_err("in-flight judge dispatch should cancel");

    assert!(matches!(err, DispatchError::Cancelled));
    assert_eq!(judge.prompt_count(), 1);
}

#[tokio::test]
async fn dispatch_clamps_out_of_range_scores() {
    let (registry, _) = make_registry(1.3);
    let config = JudgeEvaluatorConfig::default_with(registry);
    let template = make_template();
    let case = make_case();
    let invocation = make_invocation();
    let ctx = make_context(&case, &invocation);

    let outcome = dispatch_judge(&config, template, &ctx).await.unwrap();

    // Score clamped to 1.0.
    assert!((outcome.score.value - 1.0).abs() < f64::EPSILON);
    // ScoreClamped detail present with original 1.3 and clamped 1.0.
    let clamp = outcome
        .details
        .entries()
        .iter()
        .find_map(|d| match d {
            Detail::ScoreClamped { original, clamped } => Some((*original, *clamped)),
            _ => None,
        })
        .expect("ScoreClamped detail present");
    assert!((clamp.0 - 1.3).abs() < f64::EPSILON);
    assert!((clamp.1 - 1.0).abs() < f64::EPSILON);
}

#[tokio::test]
async fn dispatch_clamps_negative_scores() {
    let (registry, _) = make_registry(-0.2);
    let config = JudgeEvaluatorConfig::default_with(registry);
    let template = make_template();
    let case = make_case();
    let invocation = make_invocation();
    let ctx = make_context(&case, &invocation);

    let outcome = dispatch_judge(&config, template, &ctx).await.unwrap();

    assert!((outcome.score.value - 0.0).abs() < f64::EPSILON);
    assert!(
        outcome
            .details
            .entries()
            .iter()
            .any(|d| matches!(d, Detail::ScoreClamped { .. }))
    );
}

#[tokio::test]
async fn dispatch_uses_config_override_when_present() {
    let (registry, judge) = make_registry(0.5);
    let custom: Arc<dyn JudgePromptTemplate> = Arc::new(
        MinijinjaTemplate::new(
            "mock_v1",
            PromptFamily::Quality,
            "override Case={{ case.id }}",
        )
        .unwrap(),
    );
    let config = JudgeEvaluatorConfig::default_with(registry).with_template(custom);
    let builtin = make_template(); // would render "mock_v0" but override wins
    let case = make_case();
    let invocation = make_invocation();
    let ctx = make_context(&case, &invocation);

    let outcome = dispatch_judge(&config, builtin, &ctx).await.unwrap();

    // The recorded prompt_version must come from the override, not the builtin.
    let recorded_version = outcome
        .details
        .entries()
        .iter()
        .find_map(|d| match d {
            Detail::PromptVersion { version } => Some(version.clone()),
            _ => None,
        })
        .expect("prompt version recorded");
    assert_eq!(recorded_version, "mock_v1");

    // The judge must have seen the override prompt.
    let seen = judge.last_prompt.lock().unwrap().clone().unwrap();
    assert!(seen.starts_with("override Case=case-1"));
}

#[test]
fn detail_buffer_round_trips_through_details_string() {
    let mut buffer = DetailBuffer::new();
    buffer.push(Detail::PromptVersion {
        version: "v0".into(),
    });
    buffer.push(Detail::ScoreClamped {
        original: 1.2,
        clamped: 1.0,
    });
    let rendered = buffer.into_details_string().expect("some");
    // Two JSON lines, parseable.
    let parsed: Vec<Detail> = rendered
        .lines()
        .map(|line| serde_json::from_str::<Detail>(line).unwrap())
        .collect();
    assert_eq!(parsed.len(), 2);
    assert!(matches!(parsed[0], Detail::PromptVersion { .. }));
    assert!(matches!(parsed[1], Detail::ScoreClamped { .. }));
}

#[test]
fn empty_detail_buffer_renders_none() {
    assert!(DetailBuffer::new().into_details_string().is_none());
}

#[test]
fn evaluate_with_builtin_missing_template_returns_failed_metric() {
    let (registry, _) = make_registry(0.5);
    let config = JudgeEvaluatorConfig::default_with(registry);
    let case = make_case();
    let invocation = make_invocation();
    let ctx = make_context(&case, &invocation);

    let result = evaluate_with_builtin("missing-template-evaluator", "missing_v0", &config, &ctx);

    assert_eq!(result.evaluator_name, "missing-template-evaluator");
    assert!((result.score.value - 0.0).abs() < f64::EPSILON);
    let details = result.details.expect("missing template is reported");
    assert!(details.contains("dispatch error"));
    assert!(details.contains("built-in template missing_v0 is missing"));
}

#[test]
fn sync_bridges_work_inside_current_thread_runtime() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime");

    runtime.block_on(async {
        let block_result = block_on(async { 7usize });
        let judge_result = drive_judge_call(|| async { 11usize });

        assert_eq!(block_result, 7);
        assert_eq!(judge_result, 11);
    });
}

/// Regression: on a current-thread runtime the sync bridges offload to a
/// helper thread; the scoped judge cancellation (a thread-local installed
/// by `with_scoped_judge_cancellation`) must be carried across that hop,
/// otherwise `judge_with_retry` sees `None` and the in-flight judge
/// dispatch ignores runner cancellation.
#[test]
fn scoped_cancellation_survives_current_thread_helper_hop() {
    let (tx, rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let judge = Arc::new(PendingJudge::default());
        let registry = Arc::new(
            JudgeRegistry::builder(judge as Arc<dyn JudgeClient>, "mock-model")
                .build()
                .expect("registry builds"),
        );
        let config = JudgeEvaluatorConfig::default_with(registry);
        let case = make_case();
        let invocation = make_invocation();
        let ctx = make_context(&case, &invocation);

        // Pre-cancelled: the biased select in `judge_with_retry` must
        // observe it immediately — but only if the token survives the
        // hop onto the helper thread.
        let cancel = CancellationToken::new();
        cancel.cancel();

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread runtime");

        let results = runtime.block_on(async {
            crate::judge::with_scoped_judge_cancellation(Some(&cancel), || {
                let via_block_on = block_on(dispatch_judge(&config, make_template(), &ctx));
                let via_drive = drive_judge_call(|| async {
                    dispatch_judge(&config, make_template(), &ctx).await
                });
                (via_block_on, via_drive)
            })
        });
        let _ = tx.send(results);
    });

    // Without the fix the helper thread reads an empty thread-local, the
    // pending judge future never resolves, and the dispatch blocks
    // forever; fail fast instead of hanging the suite.
    let (via_block_on, via_drive) = rx.recv_timeout(Duration::from_secs(10)).expect(
        "judge dispatch must observe scoped cancellation \
             (thread-local lost across helper-thread hop?)",
    );

    assert!(matches!(
        via_block_on.expect_err("block_on path must cancel"),
        DispatchError::Cancelled
    ));
    assert!(matches!(
        via_drive.expect_err("drive_judge_call path must cancel"),
        DispatchError::Cancelled
    ));
}

#[test]
fn config_builder_surface() {
    let (registry, _) = make_registry(0.5);
    let config = JudgeEvaluatorConfig::default_with(registry)
        .with_system_prompt("sys")
        .with_use_reasoning(false)
        .with_feedback_key("fb");
    assert_eq!(config.system_prompt.as_deref(), Some("sys"));
    assert!(!config.use_reasoning);
    assert_eq!(config.feedback_key.as_deref(), Some("fb"));
}

#[tokio::test]
async fn dispatch_records_feedback_key_when_configured() {
    let (registry, _) = make_registry(0.8);
    let config = JudgeEvaluatorConfig::default_with(registry).with_feedback_key("quality.score");
    let template = make_template();
    let case = make_case();
    let invocation = make_invocation();
    let ctx = make_context(&case, &invocation);

    let outcome = dispatch_judge(&config, template, &ctx).await.unwrap();

    assert!(
        outcome
            .details
            .entries()
            .iter()
            .any(|d| matches!(d, Detail::FeedbackKey { key } if key == "quality.score"))
    );
}
