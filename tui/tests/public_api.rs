use std::sync::Arc;
use std::time::Duration;

use swink_agent::testing::SimpleMockStreamFn;
use swink_agent::{Agent, AgentEvent, AgentOptions, ContentBlock, ModelSpec};
use swink_agent_tui::{App, InProcessTransport, TuiConfig, TuiTransport, UserInput};

#[test]
fn tui_reexports_remain_consumable() {
    let _: fn(TuiConfig) -> App = App::new;
}

async fn recv_transport_event(transport: &mut InProcessTransport) -> AgentEvent {
    tokio::time::timeout(Duration::from_secs(3), transport.recv())
        .await
        .expect("transport should forward the agent event stream")
        .expect("agent stream should not close before AgentEnd")
}

async fn collect_turn_reply(transport: &mut InProcessTransport) -> String {
    let mut reply = String::new();

    loop {
        match recv_transport_event(transport).await {
            AgentEvent::MessageEnd { message } => {
                for block in &message.content {
                    if let ContentBlock::Text { text } = block {
                        reply.push_str(text);
                    }
                }
            }
            AgentEvent::AgentEnd { .. } => break,
            _ => {}
        }
    }

    reply
}

#[tokio::test]
async fn in_process_transport_spawn_drives_agent_events() {
    let stream = Arc::new(SimpleMockStreamFn::from_text("transport reply"));
    let options = AgentOptions::new_simple("system", ModelSpec::new("mock", "test"), stream);
    let agent = Agent::new(options);
    let mut transport = InProcessTransport::spawn(agent);

    transport
        .send(UserInput::new("hello from tui"))
        .await
        .expect("transport should accept user input");

    let mut saw_start = false;
    let mut saw_reply = false;

    loop {
        let event = recv_transport_event(&mut transport).await;

        match event {
            AgentEvent::AgentStart => saw_start = true,
            AgentEvent::MessageEnd { message } => {
                saw_reply = message.content.iter().any(|block| {
                    matches!(block, ContentBlock::Text { text } if text == "transport reply")
                });
            }
            AgentEvent::AgentEnd { .. } => break,
            _ => {}
        }
    }

    assert!(saw_start, "transport should forward AgentStart");
    assert!(saw_reply, "transport should forward the assistant reply");
}

#[tokio::test]
async fn in_process_transport_processes_queued_inputs_in_order() {
    let stream = Arc::new(SimpleMockStreamFn::from_text("queued reply"));
    let options = AgentOptions::new_simple("system", ModelSpec::new("mock", "test"), stream);
    let agent = Agent::new(options);
    let mut transport = InProcessTransport::spawn(agent);

    transport
        .send(UserInput::new("first queued prompt"))
        .await
        .expect("first input should be accepted");
    transport
        .send(UserInput::new("second queued prompt"))
        .await
        .expect("second input should be accepted");

    assert_eq!(collect_turn_reply(&mut transport).await, "queued reply");
    assert_eq!(collect_turn_reply(&mut transport).await, "queued reply");
}
