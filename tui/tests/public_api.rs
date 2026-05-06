use std::sync::Arc;
use std::time::Duration;

use swink_agent::testing::SimpleMockStreamFn;
use swink_agent::{Agent, AgentEvent, AgentOptions, ContentBlock, ModelSpec};
use swink_agent_tui::{App, InProcessTransport, TuiConfig, TuiTransport, UserInput};

#[test]
fn tui_reexports_remain_consumable() {
    let _: fn(TuiConfig) -> App = App::new;
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
        let event = tokio::time::timeout(Duration::from_secs(3), transport.recv())
            .await
            .expect("transport should forward the agent event stream")
            .expect("agent stream should not close before AgentEnd");

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
