//! US2 / T094: `EvaluationDataStore` + `LocalFileTaskResultStore` + runner wiring.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use swink_agent::{Agent, AgentOptions, ModelSpec, testing::SimpleMockStreamFn};
use swink_agent_eval::{
    AgentFactory, EvalCase, EvalError, EvalRunner, EvalSet, EvaluationDataStore, EvaluatorRegistry,
    FingerprintContext, LocalFileTaskResultStore, TaskResultCacheKey,
};

mod common;

struct CountingFactory {
    invocations: Arc<AtomicUsize>,
}

impl CountingFactory {
    fn new() -> Self {
        Self {
            invocations: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl AgentFactory for CountingFactory {
    fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        let options = AgentOptions::new_simple(
            &case.system_prompt,
            ModelSpec::new("test", "test-model"),
            Arc::new(SimpleMockStreamFn::new(vec!["ok".to_string()])),
        );
        Ok((Agent::new(options), CancellationToken::new()))
    }
    fn agent_model(&self, _: &EvalCase) -> Option<String> {
        Some("test/test-model".into())
    }
    fn tool_set_hash(&self, _: &EvalCase) -> Option<String> {
        Some("no-tools".into())
    }
}

fn set_with(cases: Vec<EvalCase>) -> EvalSet {
    EvalSet {
        id: "cache-suite".into(),
        name: "cache suite".into(),
        description: None,
        cases,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cache_hit_skips_agent_invocation() {
    let dir = TempDir::new().unwrap();
    let store: Arc<dyn EvaluationDataStore> = Arc::new(LocalFileTaskResultStore::new(dir.path()));
    let set = set_with(vec![common::make_case("c1"), common::make_case("c2")]);

    let f1 = CountingFactory::new();
    let _ = EvalRunner::new(EvaluatorRegistry::new())
        .with_cache(Arc::clone(&store))
        .run_set(&set, &f1)
        .await
        .unwrap();
    assert_eq!(f1.invocations.load(Ordering::SeqCst), 2);

    let f2 = CountingFactory::new();
    let runner2 = EvalRunner::new(EvaluatorRegistry::new()).with_cache(Arc::clone(&store));
    let _ = runner2.run_set(&set, &f2).await.unwrap();
    assert_eq!(f2.invocations.load(Ordering::SeqCst), 0);
    assert_eq!(runner2.agent_invocation_count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changing_user_messages_invalidates_cache() {
    let dir = TempDir::new().unwrap();
    let store: Arc<dyn EvaluationDataStore> = Arc::new(LocalFileTaskResultStore::new(dir.path()));

    let mut case_v2 = common::make_case("c1");
    case_v2.user_messages = vec!["totally different".into()];

    let factory = CountingFactory::new();
    let runner = EvalRunner::new(EvaluatorRegistry::new()).with_cache(Arc::clone(&store));
    let _ = runner
        .run_set(&set_with(vec![common::make_case("c1")]), &factory)
        .await
        .unwrap();
    let _ = runner
        .run_set(&set_with(vec![case_v2]), &factory)
        .await
        .unwrap();
    assert_eq!(factory.invocations.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disk_cache_round_trip_survives_restart() {
    let dir = TempDir::new().unwrap();
    let set = set_with(vec![common::make_case("c1")]);
    {
        let store: Arc<dyn EvaluationDataStore> =
            Arc::new(LocalFileTaskResultStore::new(dir.path()));
        let _ = EvalRunner::new(EvaluatorRegistry::new())
            .with_cache(store)
            .run_set(&set, &CountingFactory::new())
            .await
            .unwrap();
    }
    let store: Arc<dyn EvaluationDataStore> = Arc::new(LocalFileTaskResultStore::new(dir.path()));
    let factory = CountingFactory::new();
    let _ = EvalRunner::new(EvaluatorRegistry::new())
        .with_cache(store)
        .run_set(&set, &factory)
        .await
        .unwrap();
    assert_eq!(factory.invocations.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cached_invocation_serves_all_num_runs_iterations() {
    let dir = TempDir::new().unwrap();
    let store: Arc<dyn EvaluationDataStore> = Arc::new(LocalFileTaskResultStore::new(dir.path()));
    let set = set_with(vec![common::make_case("c1")]);

    let _ = EvalRunner::new(EvaluatorRegistry::new())
        .with_cache(Arc::clone(&store))
        .run_set(&set, &CountingFactory::new())
        .await
        .unwrap();

    let factory2 = CountingFactory::new();
    let _ = EvalRunner::new(EvaluatorRegistry::new())
        .with_cache(Arc::clone(&store))
        .with_num_runs(5)
        .run_set(&set, &factory2)
        .await
        .unwrap();
    assert_eq!(factory2.invocations.load(Ordering::SeqCst), 0);
}

#[test]
fn cache_key_is_deterministic_64_hex() {
    let fp = common::make_case("c1").content_fingerprint();
    let ctx = FingerprintContext::default();
    let k = TaskResultCacheKey::from_fingerprint(&fp, &ctx);
    assert_eq!(k.as_hex().len(), 64);
    assert_eq!(k, TaskResultCacheKey::from_fingerprint(&fp, &ctx));
}

#[test]
fn local_file_store_writes_expected_layout() {
    let dir = TempDir::new().unwrap();
    let store = LocalFileTaskResultStore::new(dir.path().to_path_buf());
    let fp = common::make_case("c1").content_fingerprint();
    let key = TaskResultCacheKey::from_fingerprint(&fp, &FingerprintContext::default());
    let invocation = common::mock_invocation(&[], Some("hi"), 0.0, 0);
    store.put("set-a", "c1", &key, &invocation).unwrap();
    let expected = dir
        .path()
        .join("set-a")
        .join("c1")
        .join(format!("{}.json", key.as_hex()));
    assert!(expected.exists());
    assert!(store.get("set-a", "c1", &key).unwrap().is_some());
}
