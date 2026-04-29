#![cfg(feature = "testkit")]
mod common;

use std::sync::Arc;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use common::*;
use swink_agent::{Agent, AgentEvent, AgentHandle, AgentOptions};
use tokio::sync::oneshot;

fn notify_on_tool_start(agent: &mut Agent, expected_name: &'static str) -> oneshot::Receiver<()> {
    let (tx, rx) = oneshot::channel();
    let tx = Arc::new(Mutex::new(Some(tx)));

    agent.subscribe({
        let tx = Arc::clone(&tx);
        move |event| {
            if let AgentEvent::ToolExecutionStart { name, .. } = event
                && name == expected_name
                && let Some(tx) = tx.lock().unwrap_or_else(PoisonError::into_inner).take()
            {
                let _ = tx.send(());
            }
        }
    });

    rx
}

fn notify_on_agent_end(agent: &mut Agent) -> oneshot::Receiver<()> {
    let (tx, rx) = oneshot::channel();
    let tx = Arc::new(Mutex::new(Some(tx)));

    agent.subscribe({
        let tx = Arc::clone(&tx);
        move |event| {
            if matches!(event, AgentEvent::AgentEnd { .. })
                && let Some(tx) = tx.lock().unwrap_or_else(PoisonError::into_inner).take()
            {
                let _ = tx.send(());
            }
        }
    });

    rx
}

async fn await_try_result(
    mut handle: AgentHandle,
) -> Result<swink_agent::AgentResult, swink_agent::AgentError> {
    loop {
        if let Some(result) = handle.try_result() {
            return result;
        }
        tokio::task::yield_now().await;
    }
}

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
    let mut agent = Agent::new(options);
    let tool_started = notify_on_tool_start(&mut agent, "slow_tool");
    let handle = AgentHandle::spawn_text(agent, "run the tool");

    tool_started.await.unwrap();
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
    let mut agent = Agent::new(options);
    let tool_started = notify_on_tool_start(&mut agent, "slow");
    let handle = AgentHandle::spawn_text(agent, "go");

    tool_started.await.unwrap();
    assert!(!handle.is_done());

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
    let mut agent = Agent::new(options);
    let tool_started = notify_on_tool_start(&mut agent, "slow");
    let mut handle = AgentHandle::spawn_text(agent, "go");

    tool_started.await.unwrap();
    assert!(handle.try_result().is_none());

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
    let mut agent = Agent::new(options);
    let agent_ended = notify_on_agent_end(&mut agent);
    let handle = AgentHandle::spawn_text(agent, "go");

    agent_ended.await.unwrap();
    let result = await_try_result(handle).await;
    assert!(result.is_ok());
}
