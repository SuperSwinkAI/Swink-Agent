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
#[path = "rate_limiter_tests.rs"]
mod tests;
