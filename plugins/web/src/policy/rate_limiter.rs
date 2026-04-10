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
    pub fn new(state: Arc<Mutex<VecDeque<Instant>>>, rate_limit_rpm: u32) -> Self {
        Self {
            state,
            rate_limit_rpm,
        }
    }
}

impl PreDispatchPolicy for RateLimitPolicy {
    fn name(&self) -> &str {
        "web.rate_limiter"
    }

    fn evaluate(&self, ctx: &mut ToolDispatchContext<'_>) -> PreDispatchVerdict {
        // Only apply to web-namespaced tools.
        if !ctx.tool_name.starts_with("web.") {
            return PreDispatchVerdict::Continue;
        }

        let mut timestamps = self.state.lock().unwrap_or_else(|e| e.into_inner());

        // Prune timestamps older than 60 seconds.
        let cutoff = Instant::now().checked_sub(Duration::from_secs(60)).unwrap();
        while timestamps.front().is_some_and(|&t| t < cutoff) {
            timestamps.pop_front();
        }

        // Check limit.
        if timestamps.len() >= self.rate_limit_rpm as usize {
            return PreDispatchVerdict::Skip(format!(
                "Rate limit exceeded: {} requests per minute",
                self.rate_limit_rpm,
            ));
        }

        timestamps.push_back(Instant::now());
        PreDispatchVerdict::Continue
    }
}
