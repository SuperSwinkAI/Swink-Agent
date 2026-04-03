#![cfg(feature = "testkit")]
mod common;

use std::sync::{Arc, Mutex};

use serde_json::json;

use common::*;
use swink_agent::{Agent, AgentEvent, AgentOptions, Emission};

#[tokio::test]
async fn emit_dispatches_to_subscribers() {
    let received: Arc<Mutex<Vec<Emission>>> = Arc::new(Mutex::new(Vec::new()));
    let received_clone = received.clone();

    let options = AgentOptions::new_simple(
        "prompt",
        default_model(),
        Arc::new(MockStreamFn::new(vec![text_only_events("hi")])),
    );
    let mut agent = Agent::new(options);

    agent.subscribe(move |event: &AgentEvent| {
        if let AgentEvent::Custom(emission) = event {
            received_clone.lock().unwrap().push(emission.clone());
        }
    });

    agent.emit("progress", json!({"step": 1}));
    agent.emit("artifact_created", json!({"path": "/tmp/out.txt"}));

    let emissions: Vec<Emission> = received.lock().unwrap().clone();
    assert_eq!(emissions.len(), 2);
    assert_eq!(emissions[0].name, "progress");
    assert_eq!(emissions[0].payload, json!({"step": 1}));
    assert_eq!(emissions[1].name, "artifact_created");
    assert_eq!(emissions[1].payload, json!({"path": "/tmp/out.txt"}));
}

#[tokio::test]
async fn emit_reaches_forwarders() {
    let received: Arc<Mutex<Vec<Emission>>> = Arc::new(Mutex::new(Vec::new()));
    let received_clone = received.clone();

    let options = AgentOptions::new_simple(
        "prompt",
        default_model(),
        Arc::new(MockStreamFn::new(vec![text_only_events("hi")])),
    )
    .with_event_forwarder(move |event: AgentEvent| {
        if let AgentEvent::Custom(emission) = event {
            received_clone.lock().unwrap().push(emission);
        }
    });

    let mut agent = Agent::new(options);

    agent.emit("my_event", json!(42));

    let emissions: Vec<Emission> = received.lock().unwrap().clone();
    assert_eq!(emissions.len(), 1);
    assert_eq!(emissions[0].name, "my_event");
    assert_eq!(emissions[0].payload, json!(42));
}
