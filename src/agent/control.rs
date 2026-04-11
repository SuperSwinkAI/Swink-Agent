use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use tokio::sync::Notify;
use tracing::info;

use super::Agent;

fn wait_for_idle_future<F>(
    notify: Arc<Notify>,
    active: Arc<std::sync::atomic::AtomicBool>,
    after_register: F,
) -> impl Future<Output = ()> + Send
where
    F: Fn() + Send + Sync + 'static,
{
    async move {
        loop {
            let notified = notify.notified();
            after_register();
            if !active.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
}

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
        wait_for_idle_future(
            Arc::clone(&self.idle_notify),
            Arc::clone(&self.loop_active),
            || {},
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::Poll;

    use futures::pin_mut;
    use tokio::sync::Notify;

    use super::wait_for_idle_future;

    #[tokio::test]
    async fn wait_for_idle_returns_when_idle_transition_happens_after_registration() {
        let notify = Arc::new(Notify::new());
        let active = Arc::new(AtomicBool::new(true));
        let active_for_hook = Arc::clone(&active);
        let notify_for_hook = Arc::clone(&notify);

        let wait_for_idle = wait_for_idle_future(notify, active, move || {
            active_for_hook.store(false, Ordering::Release);
            notify_for_hook.notify_waiters();
        });
        pin_mut!(wait_for_idle);

        assert!(matches!(futures::poll!(wait_for_idle.as_mut()), Poll::Ready(())));
    }

    #[tokio::test]
    async fn wait_for_idle_stays_pending_until_idle_notification() {
        let notify = Arc::new(Notify::new());
        let active = Arc::new(AtomicBool::new(true));
        let active_for_assert = Arc::clone(&active);

        let wait_for_idle = wait_for_idle_future(Arc::clone(&notify), Arc::clone(&active), || {});
        pin_mut!(wait_for_idle);

        assert!(matches!(futures::poll!(wait_for_idle.as_mut()), Poll::Pending));
        assert!(active_for_assert.load(Ordering::Acquire));

        active.store(false, Ordering::Release);
        notify.notify_waiters();

        assert!(matches!(futures::poll!(wait_for_idle.as_mut()), Poll::Ready(())));
    }
}
