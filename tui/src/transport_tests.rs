//! Tests for `transport`.
#![cfg(test)]

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
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), TransportError>> + Send + '_>>
    {
        Box::pin(async move { Ok(()) })
    }

    fn recv(
        &mut self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<AgentEvent>> + Send + '_>> {
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
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), TransportError>> + Send + '_>>
    {
        self.inner.send(input)
    }

    fn recv(
        &mut self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<AgentEvent>> + Send + '_>> {
        self.inner.recv()
    }

    fn try_recv(&mut self) -> Option<AgentEvent> {
        self.inner.try_recv()
    }

    fn control(
        &mut self,
        request: ControlRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ControlResponse, TransportError>> + Send + '_>,
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
