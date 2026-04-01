use std::future::Future;
use std::sync::Arc;

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
    pub fn reset(&mut self) {
        self.state.messages.clear();
        self.state.is_running = false;
        self.state.stream_message = None;
        self.state.pending_tool_calls.clear();
        self.state.error = None;
        self.abort_controller = None;
        self.in_flight_llm_messages = None;
        self.clear_queues();
    }

    /// Returns a future that resolves when the agent is no longer running.
    pub fn wait_for_idle(&self) -> impl Future<Output = ()> + Send + '_ {
        let notify = Arc::clone(&self.idle_notify);
        async move {
            if !self.state.is_running {
                return;
            }
            notify.notified().await;
        }
    }
}
