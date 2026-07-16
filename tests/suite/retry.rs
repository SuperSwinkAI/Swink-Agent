use std::io;
use std::time::Duration;
use swink_agent::{AgentError, DefaultRetryStrategy, RetryStrategy};

// -- Send + Sync compile-time assertions --------------------------------

#[test]
fn trait_object_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DefaultRetryStrategy>();
    assert_send_sync::<Box<dyn RetryStrategy>>();
}

// -- 2.7: Retries ModelThrottled up to max_attempts ---------------------

#[test]
fn retries_model_throttled_up_to_max_attempts() {
    let strategy = DefaultRetryStrategy::default().with_max_attempts(3);

    assert!(strategy.should_retry(&AgentError::ModelThrottled, 1));
    assert!(strategy.should_retry(&AgentError::ModelThrottled, 2));
    assert!(!strategy.should_retry(&AgentError::ModelThrottled, 3));
}

#[test]
fn retries_network_error_up_to_max_attempts() {
    let strategy = DefaultRetryStrategy::default().with_max_attempts(3);
    let err = AgentError::network(io::Error::other("timeout"));

    assert!(strategy.should_retry(&err, 1));
    assert!(strategy.should_retry(&err, 2));
    assert!(!strategy.should_retry(&err, 3));
}

// -- 2.8: Does not retry ContextWindowOverflow --------------------------

#[test]
fn does_not_retry_context_window_overflow() {
    let strategy = DefaultRetryStrategy::default();
    let err = AgentError::context_overflow("test-model");

    assert!(!strategy.should_retry(&err, 1));
}

#[test]
fn does_not_retry_non_retryable_variants() {
    let strategy = DefaultRetryStrategy::default();

    assert!(!strategy.should_retry(&AgentError::Aborted, 1));
    assert!(!strategy.should_retry(&AgentError::AlreadyRunning, 1));
    assert!(!strategy.should_retry(&AgentError::CacheMiss, 1));
    assert!(!strategy.should_retry(&AgentError::NoMessages, 1));
    assert!(!strategy.should_retry(&AgentError::InvalidContinue, 1));
    assert!(!strategy.should_retry(&AgentError::structured_output_failed(3, "bad"), 1));
    assert!(!strategy.should_retry(&AgentError::stream(io::Error::other("bad data")), 1));
}

// -- 2.9: Delay increases exponentially and caps at max_delay -----------

#[test]
fn delay_increases_exponentially_without_jitter() {
    let strategy = DefaultRetryStrategy::default()
        .with_base_delay(Duration::from_secs(1))
        .with_multiplier(2.0)
        .with_jitter(false);

    assert_eq!(strategy.delay(1), Duration::from_secs(1));
    assert_eq!(strategy.delay(2), Duration::from_secs(2));
    assert_eq!(strategy.delay(3), Duration::from_secs(4));
}

#[test]
fn delay_caps_at_max_delay() {
    let strategy = DefaultRetryStrategy::default()
        .with_base_delay(Duration::from_secs(1))
        .with_multiplier(2.0)
        .with_max_delay(Duration::from_secs(3))
        .with_jitter(false);

    assert_eq!(strategy.delay(1), Duration::from_secs(1));
    assert_eq!(strategy.delay(2), Duration::from_secs(2));
    assert_eq!(strategy.delay(3), Duration::from_secs(3));
}

// -- 2.10: Jitter produces varying delays -------------------------------

#[test]
fn jitter_produces_varying_delays() {
    let strategy = DefaultRetryStrategy::default()
        .with_base_delay(Duration::from_secs(10))
        .with_jitter(true);

    let delays: Vec<Duration> = (0..20).map(|_| strategy.delay(2)).collect();

    let all_same = delays.windows(2).all(|w| w[0] == w[1]);
    assert!(
        !all_same,
        "expected varying delays with jitter enabled, but all 20 samples were identical"
    );
}

// -- Builder methods ----------------------------------------------------

#[test]
fn builder_methods() {
    let strategy = DefaultRetryStrategy::default()
        .with_max_attempts(5)
        .with_base_delay(Duration::from_millis(500))
        .with_max_delay(Duration::from_secs(30))
        .with_multiplier(3.0)
        .with_jitter(false);

    assert_eq!(strategy.max_attempts, 5);
    assert_eq!(strategy.base_delay, Duration::from_millis(500));
    assert_eq!(strategy.max_delay, Duration::from_secs(30));
    assert!((strategy.multiplier - 3.0).abs() < f64::EPSILON);
    assert!(!strategy.jitter);
}

// -- T039: Jitter range validation ----------------------------------------

#[test]
fn jitter_range_within_expected_bounds() {
    let strategy = DefaultRetryStrategy::default()
        .with_base_delay(Duration::from_secs(10))
        .with_multiplier(2.0)
        .with_max_delay(Duration::from_mins(1))
        .with_jitter(true);

    // Attempt 2 without jitter would be 10 * 2^1 = 20s
    let base_delay_secs = 20.0_f64;

    for _ in 0..100 {
        let delay = strategy.delay(2);
        let secs = delay.as_secs_f64();
        // Should be within [0.5 * 20, 1.5 * 20) = [10, 30)
        assert!(
            secs >= base_delay_secs * 0.5 && secs < base_delay_secs * 1.5,
            "delay {secs}s out of expected range [{}, {})",
            base_delay_secs * 0.5,
            base_delay_secs * 1.5,
        );
    }
}

// -- T041: Custom retry strategy replaces default -------------------------

#[test]
fn custom_retry_strategy_replaces_default() {
    struct AlwaysRetry;

    impl RetryStrategy for AlwaysRetry {
        fn should_retry(&self, _error: &AgentError, _attempt: u32) -> bool {
            true
        }
        fn delay(&self, _attempt: u32) -> Duration {
            Duration::from_millis(100)
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    let strategy: Box<dyn RetryStrategy> = Box::new(AlwaysRetry);

    // Custom strategy retries even non-retryable errors
    assert!(strategy.should_retry(&AgentError::Aborted, 1));
    assert!(strategy.should_retry(&AgentError::context_overflow("test"), 1));
    assert!(strategy.should_retry(&AgentError::Aborted, 100));
    assert_eq!(strategy.delay(1), Duration::from_millis(100));
}
