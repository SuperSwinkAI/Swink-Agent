use std::sync::Arc;
use std::time::Duration;

use swink_agent_eval::{
    CacheKey, DEFAULT_JUDGE_CACHE_CAPACITY, JudgeCache, JudgeRegistry, JudgeRegistryError,
    JudgeVerdict, MockJudge, RetryPolicy, UrlFilter,
};
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

#[test]
fn judge_cache_defaults_to_spec_capacity() {
    let cache = JudgeCache::new();

    assert_eq!(cache.capacity(), DEFAULT_JUDGE_CACHE_CAPACITY);
    assert!(cache.is_empty());
}

#[test]
fn cache_key_depends_on_model_and_prompt() {
    let base = CacheKey::for_prompt("judge-a", "score this response");

    assert_eq!(base, CacheKey::for_prompt("judge-a", "score this response"));
    assert_ne!(base, CacheKey::for_prompt("judge-b", "score this response"));
    assert_ne!(
        base,
        CacheKey::for_prompt("judge-a", "score another response")
    );
}

#[test]
fn judge_cache_get_put_round_trip() {
    let mut cache = JudgeCache::with_capacity(2);
    let key = CacheKey::for_prompt("judge", "prompt");
    let verdict = verdict(0.75, true, "ok");

    cache.put(key, verdict.clone());

    assert_eq!(cache.len(), 1);
    assert_eq!(cache.get(&key), Some(verdict));
}

#[test]
fn judge_cache_replaces_existing_entry_without_growing() {
    let mut cache = JudgeCache::with_capacity(2);
    let key = CacheKey::for_prompt("judge", "prompt");

    cache.put(key, verdict(0.25, false, "first"));
    cache.put(key, verdict(0.9, true, "second"));

    assert_eq!(cache.len(), 1);
    assert_eq!(cache.get(&key), Some(verdict(0.9, true, "second")));
}

#[test]
fn judge_cache_evicts_least_recently_used_entry() {
    let mut cache = JudgeCache::with_capacity(2);
    let first = CacheKey::for_prompt("judge", "first");
    let second = CacheKey::for_prompt("judge", "second");
    let third = CacheKey::for_prompt("judge", "third");

    cache.put(first, verdict(0.1, false, "first"));
    cache.put(second, verdict(0.2, false, "second"));
    assert!(cache.get(&first).is_some(), "first becomes most recent");

    cache.put(third, verdict(0.3, true, "third"));

    assert_eq!(cache.len(), 2);
    assert!(cache.get(&second).is_none(), "second should be evicted");
    assert_eq!(cache.get(&first), Some(verdict(0.1, false, "first")));
    assert_eq!(cache.get(&third), Some(verdict(0.3, true, "third")));
}

#[test]
fn judge_cache_persists_entries_to_disk() {
    let tempdir = tempfile::tempdir().expect("temp dir");
    let key = CacheKey::for_prompt("judge", "persisted");
    let expected = verdict(0.8, true, "persisted");

    {
        let mut cache =
            JudgeCache::with_disk_path(4, tempdir.path()).expect("disk cache should open");
        assert_eq!(cache.disk_path(), Some(tempdir.path()));
        cache.put(key, expected.clone());
        cache.flush_to_disk().expect("flush succeeds");
    }

    let mut restored =
        JudgeCache::with_disk_path(4, tempdir.path()).expect("disk cache should reopen");
    assert_eq!(restored.get(&key), Some(expected));
}

#[test]
fn judge_cache_disk_flush_removes_evicted_entries() {
    let tempdir = tempfile::tempdir().expect("temp dir");
    let first = CacheKey::for_prompt("judge", "first");
    let second = CacheKey::for_prompt("judge", "second");

    {
        let mut cache =
            JudgeCache::with_disk_path(1, tempdir.path()).expect("disk cache should open");
        cache.put(first, verdict(0.1, false, "first"));
        cache.flush_to_disk().expect("first flush succeeds");
        cache.put(second, verdict(0.9, true, "second"));
        cache.flush_to_disk().expect("second flush succeeds");
    }

    let mut restored =
        JudgeCache::with_disk_path(4, tempdir.path()).expect("disk cache should reopen");
    assert!(
        restored.get(&first).is_none(),
        "evicted entry stays removed"
    );
    assert_eq!(restored.get(&second), Some(verdict(0.9, true, "second")));
}

fn verdict(score: f64, pass: bool, reason: &str) -> JudgeVerdict {
    JudgeVerdict {
        score,
        pass,
        reason: Some(reason.to_string()),
        label: None,
    }
}
