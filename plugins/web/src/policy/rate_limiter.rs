use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use swink_agent::{PreDispatchPolicy, PreDispatchVerdict, ToolDispatchContext};

/// PreDispatch policy that enforces a shared rate limit across all web tools.
///
/// Tracks request timestamps in a sliding 60-second window. When the window
/// contains `rate_limit_rpm` or more entries, subsequent web tool calls are
/// skipped until older entries expire.
pub struct RateLimitPolicy {
    state: Arc<Mutex<VecDeque<Instant>>>,
    rate_limit_rpm: u32,
}

impl RateLimitPolicy {
    const WINDOW: Duration = Duration::from_mins(1);

    pub fn new(state: Arc<Mutex<VecDeque<Instant>>>, rate_limit_rpm: u32) -> Self {
        Self {
            state,
            rate_limit_rpm,
        }
    }

    fn prune_expired(timestamps: &mut VecDeque<Instant>, now: Instant, window: Duration) {
        let Some(cutoff) = now.checked_sub(window) else {
            return;
        };

        while timestamps
            .front()
            .is_some_and(|&timestamp| timestamp < cutoff)
        {
            timestamps.pop_front();
        }
    }
}

impl PreDispatchPolicy for RateLimitPolicy {
    fn name(&self) -> &str {
        "web.rate_limiter"
    }

    fn evaluate(&self, ctx: &mut ToolDispatchContext<'_>) -> PreDispatchVerdict {
        // Only apply to web-namespaced tools.
        if !ctx.tool_name.starts_with("web_") {
            return PreDispatchVerdict::Continue;
        }

        let mut timestamps = self.state.lock().unwrap_or_else(|e| e.into_inner());

        // Prune timestamps older than 60 seconds.
        let now = Instant::now();
        Self::prune_expired(&mut timestamps, now, Self::WINDOW);

        // Check limit.
        if timestamps.len() >= self.rate_limit_rpm as usize {
            return PreDispatchVerdict::Skip(format!(
                "Rate limit exceeded: {} requests per minute",
                self.rate_limit_rpm,
            ));
        }

        timestamps.push_back(now);
        PreDispatchVerdict::Continue
    }
}

#[cfg(test)]
mod tests {
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
        ToolDispatchContext {
            tool_name,
            tool_call_id,
            arguments: args,
            execution_root: None,
            state,
        }
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
}
