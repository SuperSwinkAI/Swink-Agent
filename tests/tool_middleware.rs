//! Integration test: run a ToolMiddleware-wrapped tool through the agent loop.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use common::{
    MockStreamFn, MockTool, default_convert, default_model, text_only_events, tool_call_events,
    user_msg,
};

use swink_agent::{Agent, AgentOptions, DefaultRetryStrategy, ToolMiddleware};

#[tokio::test]
async fn middleware_runs_in_agent_loop() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = counter.clone();

    let inner = Arc::new(MockTool::new("echo"));
    let wrapped = ToolMiddleware::new(
        inner,
        move |tool, id, params, cancel, on_update, state, credential| {
            let c = counter_clone.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                tool.execute(&id, params, cancel, on_update, state, credential)
                    .await
            })
        },
    );

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("call_1", "echo", "{}"),
        text_only_events("done"),
    ]));

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_tools(vec![Arc::new(wrapped)])
            .with_retry_strategy(Box::new(
                DefaultRetryStrategy::default()
                    .with_jitter(false)
                    .with_base_delay(Duration::from_millis(1)),
            )),
    );

    let result = agent.prompt_async(vec![user_msg("hi")]).await.unwrap();

    assert!(!result.messages.is_empty());
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "middleware should have been called once"
    );
}
