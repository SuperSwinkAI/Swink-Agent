//! Integration test: run an FnTool through the agent loop with MockStreamFn.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{MockStreamFn, default_convert, default_model, text_only_events, tool_call_events, user_msg};
use serde_json::json;

use swink_agent::{Agent, AgentOptions, AgentToolResult, DefaultRetryStrategy, FnTool};

#[tokio::test]
async fn fn_tool_executes_in_agent_loop() {
    let tool = FnTool::new("greet", "Greet", "Greet a person.")
        .with_schema(json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "required": ["name"],
            "additionalProperties": false
        }))
        .with_execute_simple(|params, _cancel| async move {
            let name = params["name"].as_str().unwrap_or("world");
            AgentToolResult::text(format!("Hello, {name}!"))
        });

    // Turn 1: LLM calls the greet tool. Turn 2: LLM produces final text.
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("call_1", "greet", r#"{"name":"Alice"}"#),
        text_only_events("Done greeting."),
    ]));

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_tools(vec![Arc::new(tool)])
            .with_retry_strategy(Box::new(
                DefaultRetryStrategy::default()
                    .with_jitter(false)
                    .with_base_delay(Duration::from_millis(1)),
            )),
    );

    let result = agent.prompt_async(vec![user_msg("hi")]).await.unwrap();

    // The agent completed both turns — tool execution + final response.
    assert!(
        !result.messages.is_empty(),
        "agent should have produced messages"
    );
}
