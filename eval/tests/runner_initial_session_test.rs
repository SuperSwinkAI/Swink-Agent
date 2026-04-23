//! US2 / T101: `EvalRunner::with_initial_session_file` loader tests.

use std::fs;
use std::sync::{Arc, Mutex};

use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use swink_agent::{Agent, AgentOptions, ModelSpec, SessionState, testing::SimpleMockStreamFn};
use swink_agent_eval::{AgentFactory, EvalCase, EvalError, EvalRunner, EvalSet, EvaluatorRegistry};

mod common;

struct RecordingFactory {
    latest_session: Arc<Mutex<Option<SessionState>>>,
}

impl RecordingFactory {
    fn new() -> Self {
        Self {
            latest_session: Arc::new(Mutex::new(None)),
        }
    }
}

impl AgentFactory for RecordingFactory {
    fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError> {
        let options = AgentOptions::new_simple(
            &case.system_prompt,
            ModelSpec::new("test", "test-model"),
            Arc::new(SimpleMockStreamFn::new(vec!["ok".to_string()])),
        );
        Ok((Agent::new(options), CancellationToken::new()))
    }
    fn with_initial_session(&self, state: &SessionState) {
        *self.latest_session.lock().unwrap() = Some(state.clone());
    }
}

fn eval_set() -> EvalSet {
    EvalSet {
        id: "init-suite".into(),
        name: "init".into(),
        description: None,
        cases: vec![common::make_case("c1")],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn valid_session_file_is_surfaced_to_factory() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("initial_session.json");
    fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "data": {"greeting": "hello world"}
        }))
        .unwrap(),
    )
    .unwrap();

    let factory = RecordingFactory::new();
    let observed = Arc::clone(&factory.latest_session);
    let _ = EvalRunner::new(EvaluatorRegistry::new())
        .with_initial_session_file(path)
        .run_set(&eval_set(), &factory)
        .await
        .unwrap();

    let seen = observed
        .lock()
        .unwrap()
        .clone()
        .expect("factory should see initial session");
    let greeting: Option<String> = seen.get("greeting");
    assert_eq!(greeting.as_deref(), Some("hello world"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_file_yields_invalid_case_error() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("does-not-exist.json");
    let err = EvalRunner::new(EvaluatorRegistry::new())
        .with_initial_session_file(path)
        .run_set(&eval_set(), &RecordingFactory::new())
        .await
        .expect_err("missing file must error");
    match err {
        EvalError::InvalidCase { reason } => assert!(reason.contains("unreadable"), "{reason}"),
        other => panic!("expected InvalidCase, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_file_yields_invalid_case_error() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("initial_session.json");
    fs::write(&path, b"not valid json{{{").unwrap();
    let err = EvalRunner::new(EvaluatorRegistry::new())
        .with_initial_session_file(path)
        .run_set(&eval_set(), &RecordingFactory::new())
        .await
        .expect_err("malformed file must error");
    match err {
        EvalError::InvalidCase { reason } => assert!(reason.contains("SessionState"), "{reason}"),
        other => panic!("expected InvalidCase, got {other:?}"),
    }
}

#[test]
fn initial_session_participates_in_cache_key() {
    use swink_agent_eval::{FingerprintContext, TaskResultCacheKey};
    let fp = common::make_case("c1").content_fingerprint();
    let a = TaskResultCacheKey::from_fingerprint(&fp, &FingerprintContext::default());
    let b = TaskResultCacheKey::from_fingerprint(
        &fp,
        &FingerprintContext {
            initial_session: Some(serde_json::json!({"g": "hi"})),
            ..Default::default()
        },
    );
    assert_ne!(a, b);
}
