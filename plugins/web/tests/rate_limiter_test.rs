use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use swink_agent::policy::{PreDispatchPolicy, PreDispatchVerdict, ToolDispatchContext};
use swink_agent::SessionState;
use swink_agent_plugin_web::policy::RateLimitPolicy;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn shared_state() -> Arc<Mutex<VecDeque<Instant>>> {
    Arc::new(Mutex::new(VecDeque::new()))
}

fn make_dispatch_ctx<'a>(
    tool_name: &'a str,
    tool_call_id: &'a str,
    args: &'a mut serde_json::Value,
    state: &'a SessionState,
) -> ToolDispatchContext<'a> {
    ToolDispatchContext {
        tool_name,
        tool_call_id,
        arguments: args,
        state,
    }
}

// ---------------------------------------------------------------------------
// Requests within limit pass
// ---------------------------------------------------------------------------

#[test]
fn requests_within_limit_return_continue() {
    let rl_state = shared_state();
    let policy = RateLimitPolicy::new(rl_state, 5);
    let session = SessionState::default();

    for i in 0..5 {
        let call_id = format!("tc_{i}");
        let mut args = json!({"url": "https://example.com"});
        let mut ctx = make_dispatch_ctx("web.fetch", &call_id, &mut args, &session);
        let verdict = policy.evaluate(&mut ctx);
        assert!(
            matches!(verdict, PreDispatchVerdict::Continue),
            "request {i} should pass within limit"
        );
    }
}

// ---------------------------------------------------------------------------
// Exceeding limit returns Skip
// ---------------------------------------------------------------------------

#[test]
fn exceeding_limit_returns_skip() {
    let rl_state = shared_state();
    let policy = RateLimitPolicy::new(rl_state, 3);
    let session = SessionState::default();

    // Use up the limit.
    for i in 0..3 {
        let call_id = format!("tc_{i}");
        let mut args = json!({"url": "https://example.com"});
        let mut ctx = make_dispatch_ctx("web.fetch", &call_id, &mut args, &session);
        let verdict = policy.evaluate(&mut ctx);
        assert!(matches!(verdict, PreDispatchVerdict::Continue));
    }

    // Next request should be skipped.
    let mut args = json!({"url": "https://example.com"});
    let mut ctx = make_dispatch_ctx("web.fetch", "tc_over", &mut args, &session);
    let verdict = policy.evaluate(&mut ctx);
    assert!(
        matches!(verdict, PreDispatchVerdict::Skip(_)),
        "expected Skip when limit exceeded, got {verdict:?}"
    );
}

// ---------------------------------------------------------------------------
// After pruning old timestamps, new requests pass
// ---------------------------------------------------------------------------

#[test]
fn old_timestamps_are_pruned_allowing_new_requests() {
    let rl_state = shared_state();

    // Pre-fill with timestamps from >60 seconds ago.
    {
        let mut timestamps = rl_state.lock().unwrap();
        let old = Instant::now() - Duration::from_secs(120);
        for _ in 0..5 {
            timestamps.push_back(old);
        }
    }

    let policy = RateLimitPolicy::new(rl_state, 5);
    let session = SessionState::default();
    let mut args = json!({"url": "https://example.com"});
    let mut ctx = make_dispatch_ctx("web.fetch", "tc_after_prune", &mut args, &session);
    let verdict = policy.evaluate(&mut ctx);
    assert!(
        matches!(verdict, PreDispatchVerdict::Continue),
        "should pass after stale timestamps are pruned"
    );
}

// ---------------------------------------------------------------------------
// Non-web tool names pass through
// ---------------------------------------------------------------------------

#[test]
fn non_web_tool_returns_continue() {
    let rl_state = shared_state();
    // Even with a zero limit, non-web tools should not be affected.
    let policy = RateLimitPolicy::new(rl_state, 0);
    let session = SessionState::default();
    let mut args = json!({"command": "ls"});
    let mut ctx = make_dispatch_ctx("bash", "tc_bash", &mut args, &session);
    let verdict = policy.evaluate(&mut ctx);
    assert!(
        matches!(verdict, PreDispatchVerdict::Continue),
        "non-web tools should always pass through"
    );
}

// ---------------------------------------------------------------------------
// Default 30 RPM behavior
// ---------------------------------------------------------------------------

#[test]
fn default_30_rpm_allows_30_requests() {
    let rl_state = shared_state();
    let policy = RateLimitPolicy::new(rl_state, 30);
    let session = SessionState::default();

    for i in 0..30 {
        let call_id = format!("tc_{i}");
        let mut args = json!({"url": "https://example.com"});
        let mut ctx = make_dispatch_ctx("web.search", &call_id, &mut args, &session);
        let verdict = policy.evaluate(&mut ctx);
        assert!(
            matches!(verdict, PreDispatchVerdict::Continue),
            "request {i} of 30 should pass"
        );
    }

    // Request 31 should be rate-limited.
    let mut args = json!({"url": "https://example.com"});
    let mut ctx = make_dispatch_ctx("web.search", "tc_31", &mut args, &session);
    let verdict = policy.evaluate(&mut ctx);
    assert!(
        matches!(verdict, PreDispatchVerdict::Skip(_)),
        "31st request should be rate-limited at 30 RPM"
    );
}
