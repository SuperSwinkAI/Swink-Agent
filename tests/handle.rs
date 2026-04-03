#![cfg(feature = "testkit")]
mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use swink_agent::{Agent, AgentHandle, AgentOptions};

#[tokio::test]
async fn spawn_completes_successfully() {
    let options = AgentOptions::new_simple(
        "prompt",
        default_model(),
        Arc::new(MockStreamFn::new(vec![text_only_events("hello")])),
    );
    let agent = Agent::new(options);
    let handle = AgentHandle::spawn_text(agent, "hello");
    let result = handle.result().await.unwrap();
    assert!(!result.messages.is_empty());
}

#[tokio::test]
async fn spawn_text_convenience() {
    let options = AgentOptions::new_simple(
        "prompt",
        default_model(),
        Arc::new(MockStreamFn::new(vec![text_only_events("hi")])),
    );
    let agent = Agent::new(options);
    let handle = AgentHandle::spawn_text(agent, "test input");
    let result = handle.result().await.unwrap();
    assert!(!result.messages.is_empty());
}

#[tokio::test]
async fn cancel_running_agent() {
    let tool = Arc::new(MockTool::new("slow_tool").with_delay(Duration::from_secs(30)));
    let options = AgentOptions::new_simple(
        "prompt",
        default_model(),
        Arc::new(MockStreamFn::new(vec![
            tool_call_events("call-1", "slow_tool", "{}"),
            text_only_events("done"),
        ])),
    )
    .with_tools(vec![tool]);
    let agent = Agent::new(options);
    let handle = AgentHandle::spawn_text(agent, "run the tool");

    // Give the task a moment to start, then cancel.
    tokio::time::sleep(Duration::from_millis(50)).await;
    handle.cancel();

    let result = handle.result().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn status_transitions() {
    let options = AgentOptions::new_simple(
        "prompt",
        default_model(),
        Arc::new(MockStreamFn::new(vec![text_only_events("done")])),
    );
    let agent = Agent::new(options);
    let handle = AgentHandle::spawn_text(agent, "go");

    // Wait for completion.
    let result = handle.result().await;
    assert!(result.is_ok());
    // Status was Completed when we consumed result (verified via the Ok).
}

#[tokio::test]
async fn is_done_reflects_status() {
    let tool = Arc::new(MockTool::new("slow").with_delay(Duration::from_secs(30)));
    let options = AgentOptions::new_simple(
        "prompt",
        default_model(),
        Arc::new(MockStreamFn::new(vec![
            tool_call_events("c1", "slow", "{}"),
            text_only_events("done"),
        ])),
    )
    .with_tools(vec![tool]);
    let agent = Agent::new(options);
    let handle = AgentHandle::spawn_text(agent, "go");

    // Should not be done immediately (tool has a 30s delay).
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!handle.is_done());

    // Cancel so the test doesn't hang.
    handle.cancel();
    let _ = handle.result().await;
}

#[tokio::test]
async fn try_result_returns_none_while_running() {
    let tool = Arc::new(MockTool::new("slow").with_delay(Duration::from_secs(30)));
    let options = AgentOptions::new_simple(
        "prompt",
        default_model(),
        Arc::new(MockStreamFn::new(vec![
            tool_call_events("c1", "slow", "{}"),
            text_only_events("done"),
        ])),
    )
    .with_tools(vec![tool]);
    let agent = Agent::new(options);
    let mut handle = AgentHandle::spawn_text(agent, "go");

    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(handle.try_result().is_none());

    // Clean up.
    handle.cancel();
    let _ = handle.result().await;
}

#[tokio::test]
async fn try_result_returns_some_when_done() {
    let options = AgentOptions::new_simple(
        "prompt",
        default_model(),
        Arc::new(MockStreamFn::new(vec![text_only_events("done")])),
    );
    let agent = Agent::new(options);
    let mut handle = AgentHandle::spawn_text(agent, "go");

    // Wait for the task to finish.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let result = handle.try_result();
    assert!(result.is_some());
    assert!(result.unwrap().is_ok());
}
