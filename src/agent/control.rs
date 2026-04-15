use std::sync::Arc;
use std::sync::atomic::Ordering;

use tokio::sync::Notify;
use tracing::info;

use super::Agent;

async fn wait_for_idle_future<F>(
    notify: Arc<Notify>,
    active: Arc<std::sync::atomic::AtomicBool>,
    after_register: F,
) where
    F: Fn() + Send + Sync + 'static,
{
    loop {
        let notified = notify.notified();
        after_register();
        if !active.load(Ordering::Acquire) {
            return;
        }
        notified.await;
    }
}

impl Agent {
    pub(super) fn clear_transient_runtime_state(&mut self) {
        self.state.is_running = false;
        self.state.stream_message = None;
        self.state.pending_tool_calls.clear();
        self.state.error = None;
        self.abort_controller = None;
        self.in_flight_llm_messages = None;
        self.in_flight_messages = None;
        self.pending_message_snapshot.clear();
        self.loop_context_snapshot.clear();
    }

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
        self.loop_active.store(false, Ordering::Release);
        self.clear_transient_runtime_state();
        self.clear_queues();
        self.idle_notify.notify_waiters();
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

#[cfg(all(test, feature = "testkit"))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::Poll;

    use futures::pin_mut;
    use tokio::sync::Notify;

    use crate::agent_options::AgentOptions;
    use crate::stream::StreamFn;
    use crate::testing::{
        MockStreamFn, default_convert, default_model, text_only_events, user_msg,
    };

    use super::{Agent, wait_for_idle_future};

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

        assert!(matches!(
            futures::poll!(wait_for_idle.as_mut()),
            Poll::Ready(())
        ));
    }

    #[tokio::test]
    async fn wait_for_idle_stays_pending_until_idle_notification() {
        let notify = Arc::new(Notify::new());
        let active = Arc::new(AtomicBool::new(true));
        let active_for_assert = Arc::clone(&active);

        let wait_for_idle = wait_for_idle_future(Arc::clone(&notify), Arc::clone(&active), || {});
        pin_mut!(wait_for_idle);

        assert!(matches!(
            futures::poll!(wait_for_idle.as_mut()),
            Poll::Pending
        ));
        assert!(active_for_assert.load(Ordering::Acquire));

        active.store(false, Ordering::Release);
        notify.notify_waiters();

        assert!(matches!(
            futures::poll!(wait_for_idle.as_mut()),
            Poll::Ready(())
        ));
    }

    #[tokio::test]
    async fn reset_notifies_pending_wait_for_idle_waiters() {
        let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("done")]));
        let mut agent = Agent::new(AgentOptions::new(
            "sys",
            default_model(),
            stream_fn as Arc<dyn StreamFn>,
            default_convert,
        ));

        let _stream = agent
            .prompt_stream(vec![user_msg("hi")])
            .expect("prompt_stream should start a loop");

        let wait_for_idle = wait_for_idle_future(
            Arc::clone(&agent.idle_notify),
            Arc::clone(&agent.loop_active),
            || {},
        );
        pin_mut!(wait_for_idle);

        assert!(matches!(
            futures::poll!(wait_for_idle.as_mut()),
            Poll::Pending
        ));

        agent.reset();

        assert!(matches!(
            futures::poll!(wait_for_idle.as_mut()),
            Poll::Ready(())
        ));
    }
}
