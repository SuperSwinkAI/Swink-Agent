#![cfg(feature = "testkit")]
//! Stress test: concurrent requests to multiple orchestrated agents.

mod common;

use std::sync::Arc;
use std::time::Duration;

use swink_agent::{AgentOptions, AgentOrchestrator};

use common::{MockStreamFn, default_convert, default_model, text_events};

fn make_agent_options(name: &str) -> AgentOptions {
    let reply = format!("I am {name}");
    let responses = vec![
        text_events(&reply),
        text_events(&reply),
        text_events(&reply),
    ];
    AgentOptions::new(
        format!("You are agent {name}."),
        default_model(),
        Arc::new(MockStreamFn::new(responses)),
        default_convert,
    )
}

#[tokio::test]
async fn five_agents_three_concurrent_requests() {
    let mut orchestrator = AgentOrchestrator::new();

    // Register 5 agents.
    for i in 0..5 {
        let name = format!("agent_{i}");
        let name_clone = name.clone();
        orchestrator.add_agent(name, move || make_agent_options(&name_clone));
    }

    // Spawn 3 agents.
    let handle_0 = orchestrator.spawn("agent_0").expect("spawn agent_0");
    let handle_1 = orchestrator.spawn("agent_1").expect("spawn agent_1");
    let handle_2 = orchestrator.spawn("agent_2").expect("spawn agent_2");

    // Send concurrent requests to all 3 agents with a timeout.
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        let (r0, r1, r2) = tokio::join!(
            handle_0.send_message("hello agent_0"),
            handle_1.send_message("hello agent_1"),
            handle_2.send_message("hello agent_2"),
        );
        (r0, r1, r2)
    })
    .await;

    assert!(
        result.is_ok(),
        "orchestrator requests timed out (deadlock?)"
    );

    let (r0, r1, r2) = result.unwrap();
    assert!(r0.is_ok(), "agent_0 returned error: {:?}", r0.err());
    assert!(r1.is_ok(), "agent_1 returned error: {:?}", r1.err());
    assert!(r2.is_ok(), "agent_2 returned error: {:?}", r2.err());

    // Verify responses contain text.
    let text_0 = r0.unwrap().assistant_text();
    let text_1 = r1.unwrap().assistant_text();
    let text_2 = r2.unwrap().assistant_text();
    assert!(!text_0.is_empty(), "agent_0 produced empty response");
    assert!(!text_1.is_empty(), "agent_1 produced empty response");
    assert!(!text_2.is_empty(), "agent_2 produced empty response");

    // Verify all agents report correct status after request.
    // They should still be Running (listening for more requests).
    assert!(
        !handle_0.is_done(),
        "agent_0 should still be running after one request"
    );
    assert!(
        !handle_1.is_done(),
        "agent_1 should still be running after one request"
    );
    assert!(
        !handle_2.is_done(),
        "agent_2 should still be running after one request"
    );

    // Shut down by consuming handles.
    let final_0 = handle_0.await_result().await;
    let final_1 = handle_1.await_result().await;
    let final_2 = handle_2.await_result().await;

    // After await_result, agents should have stopped cleanly.
    // The result may be Ok or a channel-closed error — either is fine.
    // The key assertion is that we got here without deadlock.
    drop(final_0);
    drop(final_1);
    drop(final_2);
}

#[tokio::test]
async fn orchestrator_spawn_unknown_agent_errors() {
    let orchestrator = AgentOrchestrator::new();
    let result = orchestrator.spawn("nonexistent");
    assert!(result.is_err(), "spawning unregistered agent should fail");
}
