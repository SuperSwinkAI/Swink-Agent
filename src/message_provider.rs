//! Trait for polling steering and follow-up messages.
//!
//! [`MessageProvider`] replaces inline closures in [`AgentLoopConfig`](crate::loop_::AgentLoopConfig),
//! giving callers a named, testable abstraction for injecting messages into the
//! agent loop between turns.
//!
//! For push-based messaging, see [`ChannelMessageProvider`] and [`MessageSender`].

use std::sync::Mutex;

use crate::types::AgentMessage;

/// Provides steering and follow-up messages to the agent loop.
///
/// Implementors are polled at well-defined points during loop execution:
/// - [`poll_steering`](Self::poll_steering) is called after each tool execution batch.
/// - [`poll_follow_up`](Self::poll_follow_up) is called when the agent would otherwise stop.
pub trait MessageProvider: Send + Sync {
    /// Return pending steering messages, if any.
    ///
    /// Called after tool execution completes. Returning a non-empty vec causes
    /// a steering interrupt — pending tool calls may be cancelled and the new
    /// messages are injected into the conversation.
    fn poll_steering(&self) -> Vec<AgentMessage>;

    /// Return pending follow-up messages, if any.
    ///
    /// Called when the model has finished a turn and no tool calls remain.
    /// Returning a non-empty vec triggers another outer-loop iteration.
    fn poll_follow_up(&self) -> Vec<AgentMessage>;
}

/// A [`MessageProvider`] built from two closures.
///
/// Created via [`from_fns`].
pub struct FnMessageProvider<S, F>
where
    S: Fn() -> Vec<AgentMessage> + Send + Sync,
    F: Fn() -> Vec<AgentMessage> + Send + Sync,
{
    steering: S,
    follow_up: F,
}

impl<S, F> MessageProvider for FnMessageProvider<S, F>
where
    S: Fn() -> Vec<AgentMessage> + Send + Sync,
    F: Fn() -> Vec<AgentMessage> + Send + Sync,
{
    fn poll_steering(&self) -> Vec<AgentMessage> {
        (self.steering)()
    }

    fn poll_follow_up(&self) -> Vec<AgentMessage> {
        (self.follow_up)()
    }
}

/// Create a [`MessageProvider`] from two closures.
///
/// # Example
///
/// ```
/// use swink_agent::from_fns;
///
/// let provider = from_fns(
///     || vec![],  // no steering messages
///     || vec![],  // no follow-up messages
/// );
/// ```
pub const fn from_fns<S, F>(steering: S, follow_up: F) -> FnMessageProvider<S, F>
where
    S: Fn() -> Vec<AgentMessage> + Send + Sync,
    F: Fn() -> Vec<AgentMessage> + Send + Sync,
{
    FnMessageProvider {
        steering,
        follow_up,
    }
}

// ─── Channel-based MessageProvider ──────────────────────────────────────────

/// A clonable handle for pushing messages into a [`ChannelMessageProvider`].
///
/// Obtained from [`message_channel`]. Messages sent through this handle are
/// delivered as **follow-up** messages by default. Use [`send_steering`](Self::send_steering)
/// to inject steering messages instead.
#[derive(Clone)]
pub struct MessageSender {
    steering_tx: tokio::sync::mpsc::UnboundedSender<AgentMessage>,
    follow_up_tx: tokio::sync::mpsc::UnboundedSender<AgentMessage>,
}

impl MessageSender {
    /// Push a steering message to the agent.
    ///
    /// Steering messages are polled after each tool execution batch and can
    /// interrupt in-progress tool calls.
    ///
    /// Returns `false` if the receiver has been dropped.
    pub fn send_steering(&self, message: AgentMessage) -> bool {
        self.steering_tx.send(message).is_ok()
    }

    /// Push a follow-up message to the agent.
    ///
    /// Follow-up messages are polled when the agent would otherwise stop,
    /// triggering another outer-loop iteration.
    ///
    /// Returns `false` if the receiver has been dropped.
    pub fn send_follow_up(&self, message: AgentMessage) -> bool {
        self.follow_up_tx.send(message).is_ok()
    }

    /// Alias for [`send_follow_up`](Self::send_follow_up).
    pub fn send(&self, message: AgentMessage) -> bool {
        self.send_follow_up(message)
    }
}

impl std::fmt::Debug for MessageSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MessageSender").finish_non_exhaustive()
    }
}

/// A [`MessageProvider`] backed by tokio unbounded mpsc channels.
///
/// Created via [`message_channel`]. External code pushes messages through the
/// paired [`MessageSender`]; the provider drains them when the agent loop polls.
pub struct ChannelMessageProvider {
    steering_rx: Mutex<tokio::sync::mpsc::UnboundedReceiver<AgentMessage>>,
    follow_up_rx: Mutex<tokio::sync::mpsc::UnboundedReceiver<AgentMessage>>,
}

impl ChannelMessageProvider {
    /// Drain all currently buffered messages from a receiver.
    fn drain_receiver(
        rx: &Mutex<tokio::sync::mpsc::UnboundedReceiver<AgentMessage>>,
    ) -> Vec<AgentMessage> {
        let mut guard = rx.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut messages = Vec::new();
        while let Ok(msg) = guard.try_recv() {
            messages.push(msg);
        }
        messages
    }
}

impl MessageProvider for ChannelMessageProvider {
    fn poll_steering(&self) -> Vec<AgentMessage> {
        Self::drain_receiver(&self.steering_rx)
    }

    fn poll_follow_up(&self) -> Vec<AgentMessage> {
        Self::drain_receiver(&self.follow_up_rx)
    }
}

/// A [`MessageProvider`] that combines two providers, draining both on each poll.
///
/// Messages from the primary provider are returned first, followed by those
/// from the secondary provider.
pub struct ComposedMessageProvider {
    primary: std::sync::Arc<dyn MessageProvider>,
    secondary: std::sync::Arc<dyn MessageProvider>,
}

impl ComposedMessageProvider {
    /// Create a composed provider from two providers.
    pub fn new(
        primary: std::sync::Arc<dyn MessageProvider>,
        secondary: std::sync::Arc<dyn MessageProvider>,
    ) -> Self {
        Self { primary, secondary }
    }
}

impl MessageProvider for ComposedMessageProvider {
    fn poll_steering(&self) -> Vec<AgentMessage> {
        let mut msgs = self.primary.poll_steering();
        msgs.extend(self.secondary.poll_steering());
        msgs
    }

    fn poll_follow_up(&self) -> Vec<AgentMessage> {
        let mut msgs = self.primary.poll_follow_up();
        msgs.extend(self.secondary.poll_follow_up());
        msgs
    }
}

/// Create a channel-backed [`MessageProvider`] and its paired [`MessageSender`].
///
/// The returned `ChannelMessageProvider` implements [`MessageProvider`] and can
/// be passed to [`AgentLoopConfig`](crate::loop_::AgentLoopConfig) or used with
/// [`AgentOptions::with_message_channel`](crate::AgentOptions::with_message_channel).
/// The `MessageSender` is a clonable handle that external code uses to push
/// messages into the agent.
///
/// # Example
///
/// ```
/// use swink_agent::message_channel;
///
/// let (provider, sender) = message_channel();
/// // sender.send(msg) pushes a follow-up message
/// // sender.send_steering(msg) pushes a steering message
/// ```
pub fn message_channel() -> (ChannelMessageProvider, MessageSender) {
    let (steering_tx, steering_rx) = tokio::sync::mpsc::unbounded_channel();
    let (follow_up_tx, follow_up_rx) = tokio::sync::mpsc::unbounded_channel();

    let provider = ChannelMessageProvider {
        steering_rx: Mutex::new(steering_rx),
        follow_up_rx: Mutex::new(follow_up_rx),
    };

    let sender = MessageSender {
        steering_tx,
        follow_up_tx,
    };

    (provider, sender)
}
