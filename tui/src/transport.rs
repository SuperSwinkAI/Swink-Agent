//! Transport abstraction layer for the TUI.
//!
//! The [`TuiTransport`] trait decouples the TUI event loop from direct [`Agent`]
//! access.
//!
//! # Variants
//!
//! - [`InProcessTransport`] — wraps the existing in-process agent channel.
//!   Default and zero-behavior-change replacement for direct agent access.
//!
//! # Design
//!
//! The trait is intentionally minimal:
//! - [`TuiTransport::send`] accepts [`UserInput`] and forwards it to the agent.
//! - [`TuiTransport::recv`] yields [`AgentEvent`] items as they arrive, returning
//!   `None` when the stream is exhausted.

use futures::StreamExt as _;
use tokio::sync::mpsc;

use swink_agent::{Agent, AgentEvent, AgentMessage, ContentBlock, LlmMessage, UserMessage};

/// Plain text input from the user, ready to be sent to the agent.
#[derive(Debug, Clone)]
pub struct UserInput {
    /// The text content of the user's message.
    pub text: String,
}

impl UserInput {
    /// Construct a [`UserInput`] from a `String`.
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

/// Error type for transport operations.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// The underlying channel or connection has closed.
    #[error("transport channel closed")]
    ChannelClosed,

    /// Agent failed to start a prompt stream.
    #[error("failed to start agent stream: {0}")]
    StreamStart(String),

    /// I/O error on the transport connection.
    #[error("transport I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Abstraction over message exchange between the TUI and the agent backend.
///
/// Implementations forward user input to an agent and yield [`AgentEvent`]
/// notifications back to the event loop.
pub trait TuiTransport: Send {
    /// Send user input to the agent. Returns once the input has been accepted
    /// by the underlying channel or connection.
    fn send(
        &self,
        input: UserInput,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), TransportError>> + Send + '_>>;

    /// Receive the next agent event. Returns `None` when the stream is exhausted
    /// (agent finished or connection closed).
    fn recv(
        &mut self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<AgentEvent>> + Send + '_>>;

    /// Non-blocking receive: returns `Some(event)` if one is ready, otherwise `None`.
    fn try_recv(&mut self) -> Option<AgentEvent>;
}

/// In-process transport that bridges user input to an [`Agent`] running in the
/// same process.
///
/// This is the default transport. It preserves all existing behavior and is a
/// drop-in replacement for the direct channel access in the TUI event loop.
///
/// Internally it owns:
/// - An `mpsc::Sender<UserInput>` used by the event loop to enqueue input.
/// - An `mpsc::Receiver<AgentEvent>` that receives events emitted by the agent
///   loop task.
///
/// A companion task spawned by [`InProcessTransport::spawn`] reads from the
/// input channel, feeds messages to the [`Agent`], and forwards events to the
/// event receiver.
pub struct InProcessTransport {
    /// Send side: the event loop pushes user input here.
    input_tx: mpsc::Sender<UserInput>,
    /// Receive side: the event loop reads agent events here.
    event_rx: mpsc::Receiver<AgentEvent>,
}

impl InProcessTransport {
    /// Construct an `InProcessTransport` and spawn the background bridge task
    /// that drives the provided [`Agent`].
    ///
    /// The returned transport is ready to use immediately. The background task
    /// lives as long as the transport is alive (it exits when `input_tx` drops).
    pub fn spawn(mut agent: Agent) -> Self {
        let (input_tx, mut input_rx) = mpsc::channel::<UserInput>(64);
        let (event_tx, event_rx) = mpsc::channel::<AgentEvent>(256);

        tokio::spawn(async move {
            while let Some(input) = input_rx.recv().await {
                let user_msg = AgentMessage::Llm(LlmMessage::User(UserMessage {
                    content: vec![ContentBlock::Text { text: input.text }],
                    timestamp: swink_agent::now_timestamp(),
                    cache_hint: None,
                }));

                match agent.prompt_stream(vec![user_msg]) {
                    Ok(stream) => {
                        let mut stream = std::pin::pin!(stream);
                        while let Some(event) = stream.next().await {
                            if event_tx.send(event).await.is_err() {
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = event_tx
                            .send(AgentEvent::AgentEnd {
                                messages: std::sync::Arc::new(Vec::new()),
                            })
                            .await;
                        tracing::error!("InProcessTransport: agent stream error: {e}");
                    }
                }
            }
        });

        Self { input_tx, event_rx }
    }

    /// Construct an `InProcessTransport` from raw channels.
    ///
    /// Use this when you need full control over the channel pair (e.g. for
    /// testing). The caller is responsible for draining `input_rx` and feeding
    /// `event_tx`.
    pub fn from_channels(
        input_tx: mpsc::Sender<UserInput>,
        event_rx: mpsc::Receiver<AgentEvent>,
    ) -> Self {
        Self { input_tx, event_rx }
    }
}

impl TuiTransport for InProcessTransport {
    fn send(
        &self,
        input: UserInput,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), TransportError>> + Send + '_>>
    {
        let tx = self.input_tx.clone();
        Box::pin(async move {
            tx.send(input)
                .await
                .map_err(|_| TransportError::ChannelClosed)
        })
    }

    fn recv(
        &mut self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<AgentEvent>> + Send + '_>> {
        Box::pin(async move { self.event_rx.recv().await })
    }

    fn try_recv(&mut self) -> Option<AgentEvent> {
        self.event_rx.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    // ---------------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------------

    /// A minimal mock transport used to verify trait-object usage.
    struct MockTransport {
        events: Vec<AgentEvent>,
        index: usize,
    }

    impl MockTransport {
        fn new(events: Vec<AgentEvent>) -> Self {
            Self { events, index: 0 }
        }
    }

    impl TuiTransport for MockTransport {
        fn send(
            &self,
            _input: UserInput,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), TransportError>> + Send + '_>,
        > {
            Box::pin(async move { Ok(()) })
        }

        fn recv(
            &mut self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<AgentEvent>> + Send + '_>>
        {
            let event = if self.index < self.events.len() {
                let e = self.events[self.index].clone();
                self.index += 1;
                Some(e)
            } else {
                None
            };
            Box::pin(async move { event })
        }

        fn try_recv(&mut self) -> Option<AgentEvent> {
            if self.index < self.events.len() {
                let e = self.events[self.index].clone();
                self.index += 1;
                Some(e)
            } else {
                None
            }
        }
    }

    // ---------------------------------------------------------------------------
    // Tests
    // ---------------------------------------------------------------------------

    /// Verify that `UserInput::new` roundtrips the text correctly.
    #[test]
    fn user_input_roundtrip() {
        let input = UserInput::new("hello world");
        assert_eq!(input.text, "hello world");
    }

    /// Verify that `TransportError::ChannelClosed` formats as expected.
    #[test]
    fn transport_error_display() {
        let err = TransportError::ChannelClosed;
        assert_eq!(err.to_string(), "transport channel closed");
    }

    /// Verify that a mock `TuiTransport` implementation can be used as a trait object.
    #[tokio::test]
    async fn mock_transport_as_trait_object() {
        let events = vec![
            AgentEvent::AgentStart,
            AgentEvent::AgentEnd {
                messages: std::sync::Arc::new(Vec::new()),
            },
        ];
        let mut transport: Box<dyn TuiTransport> = Box::new(MockTransport::new(events));

        let result = transport.send(UserInput::new("test")).await;
        assert!(result.is_ok(), "mock send should succeed");

        let first = transport.recv().await;
        assert!(matches!(first, Some(AgentEvent::AgentStart)));

        let second = transport.recv().await;
        assert!(matches!(second, Some(AgentEvent::AgentEnd { .. })));

        let third = transport.recv().await;
        assert!(third.is_none(), "no more events");
    }

    /// Verify that `InProcessTransport::from_channels` send/recv roundtrip works.
    #[tokio::test]
    async fn in_process_transport_channel_roundtrip() {
        // Wire up the raw channels manually so the test doesn't need a real Agent.
        let (input_tx, mut input_rx) = mpsc::channel::<UserInput>(8);
        let (event_tx, event_rx) = mpsc::channel::<AgentEvent>(8);

        let mut transport = InProcessTransport::from_channels(input_tx, event_rx);

        // Send user input via the transport.
        transport
            .send(UserInput::new("hello"))
            .await
            .expect("send should succeed");

        // Verify the input arrived on the raw channel.
        let received = input_rx.recv().await.expect("input should be in channel");
        assert_eq!(received.text, "hello");

        // Inject a mock agent event through the raw event channel.
        event_tx
            .send(AgentEvent::AgentStart)
            .await
            .expect("event_tx send should succeed");

        // Receive it via the transport.
        let event = transport.recv().await;
        assert!(
            matches!(event, Some(AgentEvent::AgentStart)),
            "should receive the injected event"
        );
    }

    /// Verify that `try_recv` returns `None` when no events are queued.
    #[test]
    fn in_process_try_recv_empty() {
        let (_input_tx, _input_rx) = mpsc::channel::<UserInput>(8);
        let (_event_tx, event_rx) = mpsc::channel::<AgentEvent>(8);
        // Drop `_input_rx` — we only need the event side for this test.
        let (input_tx, _) = mpsc::channel::<UserInput>(8);
        let mut transport = InProcessTransport::from_channels(input_tx, event_rx);

        assert!(
            transport.try_recv().is_none(),
            "empty channel should return None"
        );
    }

    /// Verify that `try_recv` returns events when they are queued.
    #[tokio::test]
    async fn in_process_try_recv_with_event() {
        let (input_tx, _input_rx) = mpsc::channel::<UserInput>(8);
        let (event_tx, event_rx) = mpsc::channel::<AgentEvent>(8);
        let mut transport = InProcessTransport::from_channels(input_tx, event_rx);

        event_tx
            .send(AgentEvent::AgentStart)
            .await
            .expect("event_tx send should succeed");

        let event = transport.try_recv();
        assert!(
            matches!(event, Some(AgentEvent::AgentStart)),
            "try_recv should return queued event"
        );
    }

    /// Verify that send on a dropped receiver returns `ChannelClosed`.
    #[tokio::test]
    async fn in_process_send_channel_closed() {
        let (input_tx, input_rx) = mpsc::channel::<UserInput>(8);
        let (_event_tx, event_rx) = mpsc::channel::<AgentEvent>(8);
        let transport = InProcessTransport::from_channels(input_tx, event_rx);

        // Drop the receiver so the channel closes.
        drop(input_rx);

        let result = transport.send(UserInput::new("hello")).await;
        assert!(
            matches!(result, Err(TransportError::ChannelClosed)),
            "closed channel should return ChannelClosed error"
        );
    }
}
