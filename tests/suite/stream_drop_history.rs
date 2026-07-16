//! Dropping an un-drained event stream must not lose conversation history.
//!
//! `start_loop` moves the history into the spawned loop task; these tests
//! pin the contract that `state.messages` retains the pre-run context (plus
//! any turns already written back via `handle_stream_event`) when the
//! stream returned by `prompt_stream`/`continue_stream` is dropped before
//! `AgentEnd`, and that draining to `AgentEnd` still replaces the history
//! wholesale without duplication.

use std::sync::Arc;

use crate::common::{
    MockStreamFn, MockTool, default_convert, default_model, text_only_events, tool_call_events,
    user_msg,
};
use futures::StreamExt;

use swink_agent::{
    Agent, AgentEvent, AgentMessage, AgentOptions, AgentTool, ContentBlock, LlmMessage, StreamFn,
};

// ─── Helpers ─────────────────────────────────────────────────────────────

/// Render each message as a short tag so ordering/duplication asserts are
/// readable: `user:<text>`, `assistant`, `tool_result`.
fn message_tags(messages: &[AgentMessage]) -> Vec<String> {
    messages
        .iter()
        .map(|m| match m {
            AgentMessage::Llm(LlmMessage::User(u)) => {
                let text = u
                    .content
                    .iter()
                    .find_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .unwrap_or("");
                format!("user:{text}")
            }
            AgentMessage::Llm(LlmMessage::Assistant(_)) => "assistant".to_string(),
            AgentMessage::Llm(LlmMessage::ToolResult(_)) => "tool_result".to_string(),
            _ => "other".to_string(),
        })
        .collect()
}

fn make_agent(stream_fn: Arc<dyn StreamFn>) -> Agent {
    Agent::new(AgentOptions::new(
        "test system prompt",
        default_model(),
        stream_fn,
        default_convert,
    ))
}

// ─── Case (a): drop before any event is consumed ─────────────────────────

#[tokio::test]
async fn drop_before_first_poll_keeps_full_history() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        text_only_events("never observed"),
        text_only_events("second run"),
    ]));
    let mut agent = make_agent(stream_fn);
    agent.set_messages(vec![user_msg("m1"), user_msg("m2")]);

    let stream = agent
        .prompt_stream(vec![user_msg("new-prompt")])
        .expect("prompt_stream should start");
    // Drop without polling a single event.
    drop(stream);

    assert!(!agent.is_running(), "drop must make the agent idle");
    assert_eq!(
        message_tags(&agent.state().messages),
        vec!["user:m1", "user:m2", "user:new-prompt"],
        "the full pre-run history (including the new input) must survive"
    );

    // The agent must remain fully usable, extending the preserved history.
    let result = agent
        .prompt_async(vec![user_msg("again")])
        .await
        .expect("a new run should be allowed after dropping the old stream");
    assert!(result.error.is_none());
    assert_eq!(
        message_tags(&agent.state().messages),
        vec![
            "user:m1",
            "user:m2",
            "user:new-prompt",
            "user:again",
            "assistant"
        ],
        "the next run must build on the preserved history without duplication"
    );
}

// ─── Case (b): drop mid-stream after a partial drain ─────────────────────

#[tokio::test]
async fn drop_mid_stream_keeps_prefix_and_written_back_turns() {
    // First turn: tool call (host drains it), second turn: text (never
    // drained — the stream is dropped before it).
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("call_1", "mock_tool", "{}"),
        text_only_events("final"),
    ]));
    let tool: Arc<dyn AgentTool> = Arc::new(MockTool::new("mock_tool"));
    let mut agent = Agent::new(
        AgentOptions::new(
            "test system prompt",
            default_model(),
            stream_fn as Arc<dyn StreamFn>,
            default_convert,
        )
        .with_tools(vec![tool]),
    );
    agent.set_messages(vec![user_msg("m1")]);

    let mut stream = agent
        .prompt_stream(vec![user_msg("go")])
        .expect("prompt_stream should start");

    // Drain (and write back) events up to and including the first TurnEnd.
    while let Some(event) = stream.next().await {
        let is_turn_end = matches!(event, AgentEvent::TurnEnd { .. });
        agent.handle_stream_event(&event);
        if is_turn_end {
            break;
        }
    }
    drop(stream);

    assert!(!agent.is_running(), "drop must make the agent idle");
    assert_eq!(
        message_tags(&agent.state().messages),
        vec!["user:m1", "user:go", "assistant", "tool_result"],
        "history = original prefix + the turn already written back, \
         with no loss and no duplication"
    );
}

// ─── Case (c): drain to AgentEnd — behavior unchanged ────────────────────

#[tokio::test]
async fn drain_to_agent_end_replaces_history_without_duplication() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("done")]));
    let mut agent = make_agent(stream_fn);
    agent.set_messages(vec![user_msg("m1")]);

    let mut stream = agent
        .prompt_stream(vec![user_msg("go")])
        .expect("prompt_stream should start");
    let mut saw_agent_end = false;
    while let Some(event) = stream.next().await {
        saw_agent_end |= matches!(event, AgentEvent::AgentEnd { .. });
        agent.handle_stream_event(&event);
    }
    drop(stream);

    assert!(saw_agent_end, "the scripted run must reach AgentEnd");
    assert!(!agent.is_running());
    assert_eq!(
        message_tags(&agent.state().messages),
        vec!["user:m1", "user:go", "assistant"],
        "AgentEnd must replace the history wholesale — the pre-run snapshot \
         and incremental write-backs must not duplicate anything"
    );
}

// ─── Case (a), continue path: continue_stream also keeps history ─────────

#[tokio::test]
async fn drop_undrained_continue_stream_keeps_history() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("never observed")]));
    let mut agent = make_agent(stream_fn);
    agent.set_messages(vec![user_msg("m1"), user_msg("m2")]);

    let stream = agent
        .continue_stream()
        .expect("continue_stream should start");
    drop(stream);

    assert!(!agent.is_running(), "drop must make the agent idle");
    assert_eq!(
        message_tags(&agent.state().messages),
        vec!["user:m1", "user:m2"],
        "continue_stream moves the history into the loop too — dropping the \
         un-drained stream must not empty it"
    );
}
