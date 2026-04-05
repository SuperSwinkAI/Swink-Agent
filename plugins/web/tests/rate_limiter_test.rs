use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use swink_agent::policy::{PolicyContext, PreDispatchPolicy, PreDispatchVerdict, ToolPolicyContext};
use swink_agent::{Cost, SessionState, Usage};
use swink_agent_plugin_web::policy::RateLimitPolicy;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_policy_context() -> (Usage, Cost, SessionState) {
    (Usage::default(), Cost::default(), SessionState::default())
}

fn ctx_from<'a>(
    usage: &'a Usage,
    cost: &'a Cost,
    state: &'a SessionState,
) -> PolicyContext<'a> {
    PolicyContext {
        turn_index: 0,
        accumulated_usage: usage,
        accumulated_cost: cost,
        message_count: 0,
        overflow_signal: false,
        new_messages: &[],
        state,
    }
}

fn shared_state() -> Arc<Mutex<VecDeque<Instant>>> {
    Arc::new(Mutex::new(VecDeque::new()))
}

// ---------------------------------------------------------------------------
// Requests within limit pass
// ---------------------------------------------------------------------------

#[test]
fn requests_within_limit_return_continue() {
    let state = shared_state();
    let policy = RateLimitPolicy::new(state, 5);
    let (usage, cost, session) = make_policy_context();
    let ctx = ctx_from(&usage, &cost, &session);

    for i in 0..5 {
        let mut args = json!({"url": "https://example.com"});
        let mut tool = ToolPolicyContext {
            tool_name: "web.fetch",
            tool_call_id: &format!("tc_{i}"),
            arguments: &mut args,
        };
        let verdict = policy.evaluate(&ctx, &mut tool);
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
    let state = shared_state();
    let policy = RateLimitPolicy::new(state, 3);
    let (usage, cost, session) = make_policy_context();
    let ctx = ctx_from(&usage, &cost, &session);

    // Use up the limit.
    for i in 0..3 {
        let mut args = json!({"url": "https://example.com"});
        let mut tool = ToolPolicyContext {
            tool_name: "web.fetch",
            tool_call_id: &format!("tc_{i}"),
            arguments: &mut args,
        };
        let verdict = policy.evaluate(&ctx, &mut tool);
        assert!(matches!(verdict, PreDispatchVerdict::Continue));
    }

    // Next request should be skipped.
    let mut args = json!({"url": "https://example.com"});
    let mut tool = ToolPolicyContext {
        tool_name: "web.fetch",
        tool_call_id: "tc_over",
        arguments: &mut args,
    };
    let verdict = policy.evaluate(&ctx, &mut tool);
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
    let state = shared_state();

    // Pre-fill with timestamps from >60 seconds ago.
    {
        let mut timestamps = state.lock().unwrap();
        let old = Instant::now() - Duration::from_secs(120);
        for _ in 0..5 {
            timestamps.push_back(old);
        }
    }

    let policy = RateLimitPolicy::new(state, 5);
    let (usage, cost, session) = make_policy_context();
    let ctx = ctx_from(&usage, &cost, &session);

    let mut args = json!({"url": "https://example.com"});
    let mut tool = ToolPolicyContext {
        tool_name: "web.fetch",
        tool_call_id: "tc_after_prune",
        arguments: &mut args,
    };
    let verdict = policy.evaluate(&ctx, &mut tool);
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
    let state = shared_state();
    // Even with a zero limit, non-web tools should not be affected.
    let policy = RateLimitPolicy::new(state, 0);
    let (usage, cost, session) = make_policy_context();
    let ctx = ctx_from(&usage, &cost, &session);

    let mut args = json!({"command": "ls"});
    let mut tool = ToolPolicyContext {
        tool_name: "bash",
        tool_call_id: "tc_bash",
        arguments: &mut args,
    };
    let verdict = policy.evaluate(&ctx, &mut tool);
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
    let state = shared_state();
    let policy = RateLimitPolicy::new(state, 30);
    let (usage, cost, session) = make_policy_context();
    let ctx = ctx_from(&usage, &cost, &session);

    for i in 0..30 {
        let mut args = json!({"url": "https://example.com"});
        let mut tool = ToolPolicyContext {
            tool_name: "web.search",
            tool_call_id: &format!("tc_{i}"),
            arguments: &mut args,
        };
        let verdict = policy.evaluate(&ctx, &mut tool);
        assert!(
            matches!(verdict, PreDispatchVerdict::Continue),
            "request {i} of 30 should pass"
        );
    }

    // Request 31 should be rate-limited.
    let mut args = json!({"url": "https://example.com"});
    let mut tool = ToolPolicyContext {
        tool_name: "web.search",
        tool_call_id: "tc_31",
        arguments: &mut args,
    };
    let verdict = policy.evaluate(&ctx, &mut tool);
    assert!(
        matches!(verdict, PreDispatchVerdict::Skip(_)),
        "31st request should be rate-limited at 30 RPM"
    );
}
