//! Tests driving [`App`] through a mock [`TuiTransport`].
//!
//! The in-process bridge is covered by the rest of the suite; these tests
//! prove the transport seam works end to end: scripted events flow in through
//! `recv`, submitted prompts flow out through `send`, mention expansion still
//! happens exactly once at submit time, and steering UX is mirrored locally
//! while the backend decides what a mid-turn message means.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use swink_agent::{AgentEvent, AssistantMessage, ContentBlock, Cost, StopReason, Usage};

use crate::config::TuiConfig;
use crate::transport::{TransportError, TuiTransport, UserInput};

use super::super::*;

/// A scripted transport: yields canned events from `recv`/`try_recv` and
/// records everything sent through it.
struct ScriptedTransport {
    events: VecDeque<AgentEvent>,
    sent: Arc<Mutex<Vec<String>>>,
    fail_sends: bool,
}

impl ScriptedTransport {
    fn new(events: Vec<AgentEvent>, sent: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            events: events.into(),
            sent,
            fail_sends: false,
        }
    }
}

impl TuiTransport for ScriptedTransport {
    fn send(
        &self,
        input: UserInput,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), TransportError>> + Send + '_>> {
        if self.fail_sends {
            return Box::pin(async { Err(TransportError::channel_closed()) });
        }
        self.sent.lock().unwrap().push(input.text);
        Box::pin(async { Ok(()) })
    }

    fn recv(
        &mut self,
    ) -> Pin<Box<dyn std::future::Future<Output = Option<AgentEvent>> + Send + '_>> {
        let event = self.events.pop_front();
        Box::pin(async move { event })
    }

    fn try_recv(&mut self) -> Option<AgentEvent> {
        self.events.pop_front()
    }
}

/// An `App` wired to a `ScriptedTransport` yielding `events`, plus the shared
/// record of everything sent through the transport.
fn app_with_transport(events: Vec<AgentEvent>) -> (App, Arc<Mutex<Vec<String>>>) {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let transport = ScriptedTransport::new(events, Arc::clone(&sent));
    let app = App::new(TuiConfig::default()).with_transport(Box::new(transport));
    (app, sent)
}

/// A completed assistant response as the wire would deliver it, priced by the
/// backend before the TUI ever sees it.
fn scripted_response(text: &str, input: u64, output: u64, cost: f64) -> AgentEvent {
    AgentEvent::MessageEnd {
        message: AssistantMessage::new(
            vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            "mock",
            "mock-model",
        )
        .with_usage(Usage::default().with_input(input).with_output(output))
        .with_cost(Cost::default().with_total(cost))
        .with_stop_reason(StopReason::Stop)
        .with_timestamp(0),
    }
}

/// Scripted events pumped through `recv` drive the same state the in-process
/// event loop would produce: conversation, status, and usage accounting.
#[tokio::test]
async fn app_is_driven_through_a_mock_transport() {
    let (mut app, _sent) = app_with_transport(vec![
        AgentEvent::AgentStart,
        scripted_response("hi from the wire", 7, 3, 0.5),
        AgentEvent::AgentEnd {
            messages: Arc::new(Vec::new()),
        },
    ]);

    app.pump_transport_events().await;

    assert_eq!(app.agent_io.status, AgentStatus::Idle);
    assert_eq!(app.usage.total_input_tokens, 7);
    assert_eq!(app.usage.total_output_tokens, 3);
    assert!((app.usage.total_cost - 0.5).abs() < 1e-9);
    assert_eq!(app.usage.turn_usage.len(), 1);
    assert_eq!(app.mode.model_name, "mock-model");
    let last = app.view.messages.last().expect("an assistant message");
    assert_eq!(last.role, MessageRole::Assistant);
    assert_eq!(last.content, "hi from the wire");
}

/// Submitted input is shown locally at once, queued, and delivered through
/// `TuiTransport::send` on the event loop's flush pass.
#[tokio::test]
async fn submitted_input_flushes_through_transport_send() {
    let (mut app, sent) = app_with_transport(Vec::new());

    app.submit_user_text("hello over the wire".to_string());

    assert!(
        app.view
            .messages
            .iter()
            .any(|m| m.role == MessageRole::User && m.content == "hello over the wire"),
        "submitted text should be displayed immediately"
    );
    assert!(
        sent.lock().unwrap().is_empty(),
        "delivery is deferred to the event loop flush"
    );

    app.flush_outbound().await;

    assert_eq!(*sent.lock().unwrap(), ["hello over the wire"]);
    assert_ne!(app.agent_io.status, AgentStatus::Error);
}

/// The conversation shows the raw text the user typed; the transport carries
/// the mention-expanded text — same contract as the in-process path.
#[tokio::test]
async fn transport_send_carries_expanded_mentions() {
    let (app, sent) = app_with_transport(Vec::new());
    let mut app = app.with_extensions(
        crate::extensions::TuiExtensions::new()
            .with_mention_resolver(|_, _| Some("expanded prompt".to_string())),
    );

    app.submit_user_text("read @notes.md please".to_string());
    app.flush_outbound().await;

    assert_eq!(*sent.lock().unwrap(), ["expanded prompt"]);
    assert!(
        app.view
            .messages
            .iter()
            .any(|m| m.role == MessageRole::User && m.content == "read @notes.md please"),
        "the conversation keeps the raw text"
    );
}

/// Sending while a turn is running mirrors the in-process steering UX: the
/// text goes to the queued-message overlay, not the conversation, and the
/// backend behind the transport decides what it means.
#[tokio::test]
async fn transport_send_while_running_mirrors_the_steering_overlay() {
    let (mut app, sent) = app_with_transport(Vec::new());
    app.agent_io.status = AgentStatus::Running;

    app.submit_user_text("steer me".to_string());

    assert_eq!(app.agent_io.pending_steered, ["steer me"]);
    assert!(
        app.view.messages.iter().all(|m| m.content != "steer me"),
        "steered text is held out of the conversation until the turn boundary"
    );

    app.flush_outbound().await;

    assert_eq!(*sent.lock().unwrap(), ["steer me"]);
}

/// A failed send surfaces like an in-process start failure: an error message
/// in the conversation and `AgentStatus::Error`.
#[tokio::test]
async fn failed_transport_send_surfaces_an_error() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let mut transport = ScriptedTransport::new(Vec::new(), Arc::clone(&sent));
    transport.fail_sends = true;
    let mut app = App::new(TuiConfig::default()).with_transport(Box::new(transport));

    app.submit_user_text("doomed".to_string());
    app.flush_outbound().await;

    assert_eq!(app.agent_io.status, AgentStatus::Error);
    assert!(
        app.view
            .messages
            .iter()
            .any(|m| m.role == MessageRole::Error && m.content.contains("Failed to send to agent")),
        "the failure should be visible in the conversation"
    );
    assert!(sent.lock().unwrap().is_empty());
}

/// Without a transport installed (and no agent set), submitting is still the
/// no-op it always was — nothing queues for a transport nobody installed.
#[test]
fn default_in_process_app_does_not_queue_outbound() {
    let mut app = App::new(TuiConfig::default());

    app.send_to_agent("typed before any agent exists".to_string());

    assert!(app.agent_io.outbound.is_empty());
    assert_eq!(app.agent_io.status, AgentStatus::Idle);
}
