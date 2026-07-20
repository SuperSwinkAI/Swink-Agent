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
//! The trait separates turn I/O from control:
//! - [`TuiTransport::send`] accepts [`UserInput`] and forwards it to the agent.
//! - [`TuiTransport::recv`] yields [`AgentEvent`] items as they arrive, returning
//!   `None` when the stream is exhausted.
//! - [`TuiTransport::control`] carries everything out-of-band to a turn —
//!   abort, model selection, thinking level, approval mode, system prompt,
//!   reset, plan mode, and session snapshot/restore — as a
//!   [`ControlRequest`] → [`ControlResponse`] round trip. It has a default
//!   implementation returning [`TransportError::Unsupported`], so a
//!   turn-I/O-only transport is still a complete implementation: the
//!   [`App`](crate::App) degrades to a status notice (or a silent skip, for
//!   auto-save) when the backend cannot service a control request.
//!
//! # How the app consumes this
//!
//! [`App`](crate::App) always reads agent events through a boxed
//! `TuiTransport`. By default that is an [`InProcessTransport`] wrapped
//! around the internal event channel the in-process agent bridge feeds, and
//! prompts are started on the [`Agent`] directly — behavior is unchanged. A
//! host can replace the wiring with
//! [`App::with_transport`](crate::App::with_transport), after which submitted
//! prompts are delivered through [`TuiTransport::send`], the backend on
//! the other side of the transport (e.g. a remote agent service) owns the
//! turn lifecycle, and control operations (Ctrl-C abort, F4 model cycling,
//! `#approval`/`#system`/`#reset`, plan mode, session save/load) are issued
//! through [`TuiTransport::control`] instead of a local [`Agent`] handle.
//! [`App::pump_transport_events`](crate::App::pump_transport_events)
//! drives an `App` from a transport without a terminal, which is what a mock
//! transport test wants.

use futures::StreamExt as _;
use tokio::sync::mpsc;

use swink_agent::{
    Agent, AgentEvent, AgentMessage, ApprovalMode, CompactionReport, ContentBlock, LlmMessage,
    ModelSpec, ThinkingLevel, UserMessage,
};

/// Plain text input from the user, ready to be sent to the agent.
#[non_exhaustive]
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

/// A control-plane request from the TUI to the agent backend.
///
/// Everything here is out-of-band relative to turn I/O
/// ([`TuiTransport::send`] / [`TuiTransport::recv`]): aborting a running
/// turn, switching models, changing modes, and moving session snapshots.
/// Issued through [`TuiTransport::control`]; the expected reply shape for
/// each variant is documented on [`ControlResponse`].
#[non_exhaustive]
#[derive(Debug)]
pub enum ControlRequest {
    /// Abort the running turn, if any. Expects [`ControlResponse::Ack`].
    Abort,
    /// List the models the backend can switch between. Expects
    /// [`ControlResponse::Models`].
    ListModels,
    /// Switch the model used for subsequent turns. Expects
    /// [`ControlResponse::Ack`].
    SetModel(ModelSpec),
    /// Set the thinking level for subsequent turns. Expects
    /// [`ControlResponse::Ack`].
    SetThinkingLevel(ThinkingLevel),
    /// Set the tool approval mode. Expects [`ControlResponse::Ack`].
    SetApprovalMode(ApprovalMode),
    /// Ask which tool approval mode is active. Expects
    /// [`ControlResponse::ApprovalMode`].
    QueryApprovalMode,
    /// Replace the system prompt. Expects [`ControlResponse::Ack`].
    SetSystemPrompt(String),
    /// Reset the agent's conversation state. Expects
    /// [`ControlResponse::Ack`].
    Reset,
    /// Run the backend's context transformers against the stored history now
    /// (manual compaction, e.g. a `/compact` command). Idle-only: backends
    /// must refuse while a turn is in flight. Expects
    /// [`ControlResponse::Compacted`].
    Compact,
    /// Enter plan mode (read-only tools). The backend owns the saved tool
    /// set and system prompt for the round trip. Expects
    /// [`ControlResponse::Ack`].
    EnterPlanMode,
    /// Exit plan mode, restoring the tool set and system prompt the backend
    /// saved on [`ControlRequest::EnterPlanMode`]. Expects
    /// [`ControlResponse::Ack`].
    ExitPlanMode,
    /// Ask for the backend's current transcript and session state, e.g. to
    /// persist them client-side. Expects
    /// [`ControlResponse::SessionSnapshot`].
    SnapshotSession,
    /// Replace the backend's transcript and session state, e.g. after the
    /// client loaded a saved session. Expects [`ControlResponse::Ack`].
    RestoreSession {
        /// Full transcript to install.
        messages: Vec<AgentMessage>,
        /// Session state snapshot (see
        /// [`SessionState::snapshot`](swink_agent::SessionState::snapshot)),
        /// or `None` to reset the state.
        state: Option<serde_json::Value>,
    },
}

/// A successful reply to a [`ControlRequest`].
#[non_exhaustive]
#[derive(Debug)]
pub enum ControlResponse {
    /// The request was applied; there is nothing to return.
    Ack,
    /// Reply to [`ControlRequest::ListModels`].
    Models {
        /// Models the backend can switch between.
        available: Vec<ModelSpec>,
        /// The model currently in use.
        current: ModelSpec,
    },
    /// Reply to [`ControlRequest::QueryApprovalMode`].
    ApprovalMode(ApprovalMode),
    /// Reply to [`ControlRequest::SnapshotSession`].
    SessionSnapshot {
        /// The backend's current transcript.
        messages: Vec<AgentMessage>,
        /// Session state snapshot, or `None` if the backend has none.
        state: Option<serde_json::Value>,
    },
    /// Reply to [`ControlRequest::Compact`].
    Compacted {
        /// The last transformer's report, or `None` when no transformer is
        /// configured or every transformer declined (history under budget).
        report: Option<CompactionReport>,
    },
}

/// Error type for transport operations.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// The underlying channel or connection has closed.
    #[error("transport channel closed")]
    ChannelClosed,

    /// Agent failed to start a prompt stream.
    #[error("failed to start agent stream: {0}")]
    StreamStart(String),

    /// The transport does not support this operation.
    #[error("operation not supported by this transport")]
    Unsupported,

    /// I/O error on the transport connection.
    #[error("transport I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl TransportError {
    /// Create a [`TransportError::ChannelClosed`].
    ///
    /// For [`TuiTransport`] implementations reporting that their underlying
    /// channel or connection has closed.
    #[must_use]
    pub const fn channel_closed() -> Self {
        Self::ChannelClosed
    }

    /// Create a [`TransportError::StreamStart`] with the given reason.
    ///
    /// For [`TuiTransport`] implementations reporting a failure to start the
    /// agent prompt stream.
    #[must_use]
    pub fn stream_start(reason: impl Into<String>) -> Self {
        Self::StreamStart(reason.into())
    }

    /// Create a [`TransportError::Unsupported`].
    ///
    /// For [`TuiTransport`] implementations — including the default
    /// [`TuiTransport::control`] — reporting that the requested operation is
    /// not supported by this transport.
    #[must_use]
    pub const fn unsupported() -> Self {
        Self::Unsupported
    }
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

    /// Issue a control-plane request and await the backend's response.
    ///
    /// Control requests are out-of-band relative to turn I/O: aborting a
    /// running turn, switching models, changing approval or plan mode, and
    /// moving session snapshots (see [`ControlRequest`]). The default
    /// implementation rejects every request with
    /// [`TransportError::Unsupported`], so existing turn-I/O-only transports
    /// keep compiling — the [`App`](crate::App) degrades gracefully by
    /// surfacing an "unsupported" notice (or silently skipping auto-save)
    /// instead of pretending the operation happened.
    ///
    /// [`InProcessTransport`] deliberately keeps the default: in in-process
    /// mode the `App` drives the [`Agent`] directly and never calls this.
    fn control(
        &mut self,
        request: ControlRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ControlResponse, TransportError>> + Send + '_>,
    > {
        let _ = request;
        Box::pin(async { Err(TransportError::unsupported()) })
    }
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
                let user_msg = AgentMessage::Llm(LlmMessage::User(
                    UserMessage::new(vec![ContentBlock::Text { text: input.text }])
                        .with_timestamp(swink_agent::now_timestamp()),
                ));

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

    #[test]
    fn transport_error_constructors_build_expected_variants() {
        assert!(matches!(
            TransportError::channel_closed(),
            TransportError::ChannelClosed
        ));
        match TransportError::stream_start("boom") {
            TransportError::StreamStart(reason) => assert_eq!(reason, "boom"),
            other => panic!("unexpected variant: {other:?}"),
        }
        assert!(matches!(
            TransportError::unsupported(),
            TransportError::Unsupported
        ));
    }

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

    /// Verify that `TransportError::Unsupported` formats as expected.
    #[test]
    fn transport_error_unsupported_display() {
        let err = TransportError::Unsupported;
        assert_eq!(err.to_string(), "operation not supported by this transport");
    }

    /// A transport that does not override `control` rejects every request
    /// with `Unsupported` — the seam is opt-in.
    #[tokio::test]
    async fn default_control_returns_unsupported() {
        let mut transport: Box<dyn TuiTransport> = Box::new(MockTransport::new(Vec::new()));

        let result = transport.control(ControlRequest::Abort).await;
        assert!(matches!(result, Err(TransportError::Unsupported)));

        let result = transport.control(ControlRequest::ListModels).await;
        assert!(matches!(result, Err(TransportError::Unsupported)));
    }

    /// A mock that records every control request and replies from a script.
    struct RecordingControlTransport {
        inner: MockTransport,
        requests: Vec<ControlRequest>,
        responses: std::collections::VecDeque<Result<ControlResponse, TransportError>>,
    }

    impl TuiTransport for RecordingControlTransport {
        fn send(
            &self,
            input: UserInput,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), TransportError>> + Send + '_>,
        > {
            self.inner.send(input)
        }

        fn recv(
            &mut self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<AgentEvent>> + Send + '_>>
        {
            self.inner.recv()
        }

        fn try_recv(&mut self) -> Option<AgentEvent> {
            self.inner.try_recv()
        }

        fn control(
            &mut self,
            request: ControlRequest,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<ControlResponse, TransportError>>
                    + Send
                    + '_,
            >,
        > {
            self.requests.push(request);
            let response = self
                .responses
                .pop_front()
                .unwrap_or_else(|| Err(TransportError::Unsupported));
            Box::pin(async move { response })
        }
    }

    /// Overriding `control` lets an implementation observe requests and
    /// script responses — the contract the App-side routing relies on.
    #[tokio::test]
    async fn overridden_control_records_requests_and_scripts_responses() {
        let mut transport = RecordingControlTransport {
            inner: MockTransport::new(Vec::new()),
            requests: Vec::new(),
            responses: [
                Ok(ControlResponse::Ack),
                Ok(ControlResponse::ApprovalMode(ApprovalMode::Smart)),
            ]
            .into(),
        };

        let first = transport.control(ControlRequest::Abort).await;
        assert!(matches!(first, Ok(ControlResponse::Ack)));

        let second = transport.control(ControlRequest::QueryApprovalMode).await;
        assert!(matches!(
            second,
            Ok(ControlResponse::ApprovalMode(ApprovalMode::Smart))
        ));

        assert!(matches!(
            transport.requests[..],
            [ControlRequest::Abort, ControlRequest::QueryApprovalMode]
        ));

        // Script exhausted: falls back to Unsupported.
        let third = transport.control(ControlRequest::Reset).await;
        assert!(matches!(third, Err(TransportError::Unsupported)));
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
