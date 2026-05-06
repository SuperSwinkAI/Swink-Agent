#![cfg(feature = "testkit")]
mod common;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use common::*;
use swink_agent::{Agent, AgentEvent, AgentOptions};

#[tokio::test]
async fn forwarder_receives_events() {
    let events_received: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events_received.clone();

    let options = AgentOptions::new_simple(
        "prompt",
        default_model(),
        Arc::new(MockStreamFn::new(vec![text_only_events("hello")])),
    )
    .with_event_forwarder(move |event: AgentEvent| {
        let label = match &event {
            AgentEvent::AgentStart => "AgentStart".to_string(),
            AgentEvent::AgentEnd { .. } => "AgentEnd".to_string(),
            AgentEvent::TurnStart => "TurnStart".to_string(),
            AgentEvent::TurnEnd { .. } => "TurnEnd".to_string(),
            AgentEvent::MessageStart => "MessageStart".to_string(),
            AgentEvent::MessageEnd { .. } => "MessageEnd".to_string(),
            _ => format!("{event:?}"),
        };
        events_clone.lock().unwrap().push(label);
    });

    let mut agent = Agent::new(options);
    let _ = agent.prompt_text("hello").await;

    let received: Vec<String> = events_received.lock().unwrap().clone();
    assert!(
        received.contains(&"AgentStart".to_string()),
        "forwarder should receive AgentStart"
    );
    assert!(
        received.contains(&"AgentEnd".to_string()),
        "forwarder should receive AgentEnd"
    );
}

#[tokio::test]
async fn multiple_forwarders() {
    let counter_a: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let counter_b: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let a = counter_a.clone();
    let b = counter_b.clone();

    let options = AgentOptions::new_simple(
        "prompt",
        default_model(),
        Arc::new(MockStreamFn::new(vec![text_only_events("hi")])),
    )
    .with_event_forwarder(move |_event: AgentEvent| {
        *a.lock().unwrap() += 1;
    })
    .with_event_forwarder(move |_event: AgentEvent| {
        *b.lock().unwrap() += 1;
    });

    let mut agent = Agent::new(options);
    let _ = agent.prompt_text("hello").await;

    let a_count = *counter_a.lock().unwrap();
    let b_count = *counter_b.lock().unwrap();
    assert!(a_count > 0, "forwarder A should receive events");
    assert!(b_count > 0, "forwarder B should receive events");
    assert_eq!(
        a_count, b_count,
        "both forwarders should receive the same number of events"
    );
}

#[tokio::test]
async fn add_forwarder_at_runtime() {
    let events_received: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events_received.clone();

    let options = AgentOptions::new_simple(
        "prompt",
        default_model(),
        Arc::new(MockStreamFn::new(vec![text_only_events("hi")])),
    );
    let mut agent = Agent::new(options);

    agent.add_event_forwarder(move |event: AgentEvent| {
        if matches!(&event, AgentEvent::AgentStart | AgentEvent::AgentEnd { .. }) {
            let label = match &event {
                AgentEvent::AgentStart => "AgentStart",
                AgentEvent::AgentEnd { .. } => "AgentEnd",
                _ => unreachable!(),
            };
            events_clone.lock().unwrap().push(label.to_string());
        }
    });

    let _ = agent.prompt_text("hello").await;

    let received: Vec<String> = events_received.lock().unwrap().clone();
    assert!(
        received.contains(&"AgentStart".to_string()),
        "runtime forwarder should receive AgentStart"
    );
    assert!(
        received.contains(&"AgentEnd".to_string()),
        "runtime forwarder should receive AgentEnd"
    );
}

#[tokio::test]
async fn panicking_forwarder_is_removed_after_first_panic() {
    let panicking_calls = Arc::new(AtomicUsize::new(0));
    let healthy_calls = Arc::new(AtomicUsize::new(0));
    let panicking_calls_clone = Arc::clone(&panicking_calls);
    let healthy_calls_clone = Arc::clone(&healthy_calls);

    let options = AgentOptions::new_simple(
        "prompt",
        default_model(),
        Arc::new(MockStreamFn::new(vec![text_only_events("hi")])),
    )
    .with_event_forwarder(move |_event: AgentEvent| {
        panicking_calls_clone.fetch_add(1, Ordering::SeqCst);
        panic!("forwarder failed");
    })
    .with_event_forwarder(move |_event: AgentEvent| {
        healthy_calls_clone.fetch_add(1, Ordering::SeqCst);
    });
    let mut agent = Agent::new(options);

    agent.forward_event(&AgentEvent::TurnStart);
    agent.forward_event(&AgentEvent::AgentStart);

    assert_eq!(panicking_calls.load(Ordering::SeqCst), 1);
    assert_eq!(healthy_calls.load(Ordering::SeqCst), 2);
}
