use std::sync::Arc;
use std::time::Duration;

use swink_agent_eval::{JudgeRegistry, JudgeRegistryError, MockJudge, RetryPolicy, UrlFilter};
use url::Url;

#[test]
fn default_retry_policy_matches_spec_bounds() {
    let policy = RetryPolicy::default();

    assert_eq!(policy.max_attempts, 6);
    assert_eq!(policy.max_delay, Duration::from_secs(240));
    assert!(policy.jitter);
}

#[test]
fn registry_requires_explicit_model_id() {
    let err = JudgeRegistry::builder(Arc::new(MockJudge::always_pass()), "  ")
        .build()
        .expect_err("empty model id must be rejected");

    assert_eq!(err, JudgeRegistryError::MissingModelId);
}

#[test]
fn registry_trims_and_stores_model_id() {
    let registry = JudgeRegistry::builder(Arc::new(MockJudge::always_pass()), "  claude-test  ")
        .build()
        .expect("valid registry");

    assert_eq!(registry.model_id(), "claude-test");
    assert_eq!(registry.batch_size(), 1);
    assert_eq!(registry.retry_policy(), &RetryPolicy::default());
}

#[test]
fn registry_rejects_batch_size_outside_allowed_range() {
    for batch_size in [0, 129] {
        let err = JudgeRegistry::builder(Arc::new(MockJudge::always_pass()), "model")
            .with_batch_size(batch_size)
            .build()
            .expect_err("invalid batch size must be rejected");

        assert_eq!(err, JudgeRegistryError::InvalidBatchSize { batch_size });
    }
}

#[test]
fn registry_accepts_batch_size_bounds() {
    for batch_size in [1, 128] {
        let registry = JudgeRegistry::builder(Arc::new(MockJudge::always_pass()), "model")
            .with_batch_size(batch_size)
            .build()
            .expect("boundary batch size should be accepted");

        assert_eq!(registry.batch_size(), batch_size);
    }
}

#[test]
fn registry_rejects_invalid_retry_attempts() {
    for max_attempts in [0, 17] {
        let err = JudgeRegistry::builder(Arc::new(MockJudge::always_pass()), "model")
            .with_retry_policy(RetryPolicy::new(
                max_attempts,
                Duration::from_secs(1),
                false,
            ))
            .build()
            .expect_err("invalid retry attempts must be rejected");

        assert!(matches!(err, JudgeRegistryError::InvalidRetryPolicy { .. }));
    }
}

#[test]
fn registry_accepts_custom_url_filter() {
    #[derive(Debug)]
    struct AllowAll;

    impl UrlFilter for AllowAll {
        fn allows(&self, _url: &Url) -> bool {
            true
        }
    }

    let registry = JudgeRegistry::builder(Arc::new(MockJudge::always_pass()), "model")
        .with_url_filter(Arc::new(AllowAll))
        .build()
        .expect("valid registry");

    let url = Url::parse("http://127.0.0.1/fixture.png").expect("valid url");
    assert!(registry.url_filter().allows(&url));
}
