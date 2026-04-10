use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use tracing::info;

use super::Agent;

impl Agent {
    /// Cancel the currently running loop, if any.
    pub fn abort(&mut self) {
        if let Some(ref token) = self.abort_controller {
            info!("aborting agent loop");
            token.cancel();
        }
    }

    /// Reset the agent to its initial state, clearing messages, queues, and error.
    ///
    /// If a loop is currently active, the abort token is cancelled and the
    /// generation counter is bumped so the stale [`LoopGuardStream`] cannot
    /// clear `loop_active` for any future run.
    pub fn reset(&mut self) {
        // Cancel the running loop *before* dropping the token, so the spawned
        // stream observes cancellation rather than continuing to emit events.
        if let Some(ref token) = self.abort_controller {
            token.cancel();
        }

        // Bump the generation counter so the old LoopGuardStream's Drop impl
        // sees a mismatched generation and skips clearing loop_active.
        self.loop_generation.fetch_add(1, Ordering::AcqRel);

        self.state.messages.clear();
        self.state.is_running = false;
        self.loop_active.store(false, Ordering::Release);
        self.state.stream_message = None;
        self.state.pending_tool_calls.clear();
        self.state.error = None;
        self.abort_controller = None;
        self.in_flight_llm_messages = None;
        self.clear_queues();
    }

    /// Returns a future that resolves when the agent is no longer running.
    ///
    /// Uses the shared `loop_active` flag so the future correctly resolves even
    /// when the event stream is dropped without being drained to `AgentEnd`.
    pub fn wait_for_idle(&self) -> impl Future<Output = ()> + Send + '_ {
        let notify = Arc::clone(&self.idle_notify);
        let active = Arc::clone(&self.loop_active);
        async move {
            if !active.load(Ordering::Acquire) {
                return;
            }
            notify.notified().await;
        }
    }
}
