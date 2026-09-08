//! Tests for `rate_limiter`.
#![cfg(test)]

use serde_json::json;
use swink_agent::SessionState;

use super::*;

fn shared_state() -> Arc<Mutex<VecDeque<Instant>>> {
    Arc::new(Mutex::new(VecDeque::new()))
}

fn make_dispatch_ctx<'a>(
    tool_name: &'a str,
    tool_call_id: &'a str,
    args: &'a mut serde_json::Value,
    state: &'a SessionState,
) -> ToolDispatchContext<'a> {
    ToolDispatchContext::new(tool_name, tool_call_id, args, None, state)
}

#[test]
fn requests_within_limit_return_continue() {
    let policy = RateLimitPolicy::new(shared_state(), 5);
    let session = SessionState::default();

    for i in 0..5 {
        let call_id = format!("tc_{i}");
        let mut args = json!({"url": "https://example.com"});
        let mut ctx = make_dispatch_ctx("web_fetch", &call_id, &mut args, &session);
        assert!(matches!(
            policy.evaluate(&mut ctx),
            PreDispatchVerdict::Continue
        ));
    }
}

#[test]
fn exceeding_limit_returns_skip() {
    let policy = RateLimitPolicy::new(shared_state(), 3);
    let session = SessionState::default();

    for i in 0..3 {
        let call_id = format!("tc_{i}");
        let mut args = json!({"url": "https://example.com"});
        let mut ctx = make_dispatch_ctx("web_fetch", &call_id, &mut args, &session);
        assert!(matches!(
            policy.evaluate(&mut ctx),
            PreDispatchVerdict::Continue
        ));
    }

    let mut args = json!({"url": "https://example.com"});
    let mut ctx = make_dispatch_ctx("web_fetch", "tc_over", &mut args, &session);
    assert!(matches!(
        policy.evaluate(&mut ctx),
        PreDispatchVerdict::Skip(_)
    ));
}

#[test]
fn old_timestamps_are_pruned_allowing_new_requests() {
    let rl_state = shared_state();
    {
        let mut timestamps = rl_state.lock().unwrap();
        let old = Instant::now();
        for _ in 0..5 {
            timestamps.push_back(old);
        }
    }

    let now = rl_state
        .lock()
        .unwrap()
        .front()
        .copied()
        .and_then(|timestamp| timestamp.checked_add(Duration::from_mins(2)))
        .expect("timestamp + 120s should remain representable");
    {
        let mut timestamps = rl_state.lock().unwrap();
        RateLimitPolicy::prune_expired(&mut timestamps, now, Duration::from_mins(1));
    }

    let policy = RateLimitPolicy::new(rl_state, 5);
    let session = SessionState::default();
    let mut args = json!({"url": "https://example.com"});
    let mut ctx = make_dispatch_ctx("web_fetch", "tc_after_prune", &mut args, &session);
    assert!(matches!(
        policy.evaluate(&mut ctx),
        PreDispatchVerdict::Continue
    ));
}

#[test]
fn underflowing_cutoff_does_not_prune_or_panic() {
    let now = Instant::now();
    let mut timestamps = VecDeque::from([now]);

    RateLimitPolicy::prune_expired(&mut timestamps, now, Duration::MAX);

    assert_eq!(timestamps.len(), 1);
}
