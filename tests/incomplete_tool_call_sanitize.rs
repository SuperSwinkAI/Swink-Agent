//! Regression tests for issue #619: provider adapters forward incomplete
//! `tool_use` arguments after `StopReason::Length` truncation, causing API 400
//! on the next turn.
//!
//! The loop-level scrub (`sanitize_incomplete_tool_calls`) runs before the
//! assistant message is committed to `context_messages`. After it runs, every
//! `ContentBlock::ToolCall` in the history must have an object-typed
//! `arguments` field with no `partial_json` set.

#![cfg(feature = "testkit")]
mod common;

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use common::{MockStreamFn, default_model};
use futures::Stream;
use futures::stream::StreamExt;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use swink_agent::{
    AgentEvent, AgentLoopConfig, AgentMessage, AgentTool, AgentToolResult, AssistantMessageEvent,
    ContentBlock, Cost, DefaultRetryStrategy, LlmMessage, StopReason, StreamFn, StreamOptions,
    Usage, UserMessage, agent_loop,
};

// ─── Minimal in-memory tool ────────────────────────────────────────────────

struct EchoTool;

impl AgentTool for EchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn label(&self) -> &'static str {
        "echo"
    }

    fn description(&self) -> &'static str {
        "echoes its arguments"
    }

    fn parameters_schema(&self) -> &serde_json::Value {
        static SCHEMA: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| {
            json!({
                "type": "object",
                "properties": {"msg": {"type": "string"}},
                "additionalProperties": true
            })
        })
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        _cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: Arc<std::sync::RwLock<swink_agent::SessionState>>,
        _credential: Option<swink_agent::ResolvedCredential>,
    ) -> Pin<Box<dyn std::future::Future<Output = AgentToolResult> + Send + '_>> {
        Box::pin(async move { AgentToolResult::text(format!("echo: {params}")) })
    }
}

type ConvertToLlmBoxed = Box<dyn Fn(&AgentMessage) -> Option<LlmMessage> + Send + Sync>;

fn default_convert_to_llm() -> ConvertToLlmBoxed {
    Box::new(|msg: &AgentMessage| match msg {
        AgentMessage::Llm(llm) => Some(llm.clone()),
        AgentMessage::Custom(_) => None,
    })
}

fn loop_config(stream_fn: Arc<dyn StreamFn>, tool: Arc<dyn AgentTool>) -> AgentLoopConfig {
    AgentLoopConfig {
        agent_name: None,
        transfer_chain: None,
        model: default_model(),
        stream_options: StreamOptions::default(),
        retry_strategy: Box::new(
            DefaultRetryStrategy::default()
                .with_jitter(false)
                .with_base_delay(Duration::from_millis(1)),
        ),
        stream_fn,
        tools: vec![tool],
        convert_to_llm: default_convert_to_llm(),
        transform_context: None,
        get_api_key: None,
        message_provider: None,
        pending_message_snapshot: Arc::default(),
        loop_context_snapshot: Arc::default(),
        approve_tool: None,
        approval_mode: swink_agent::ApprovalMode::default(),
        pre_turn_policies: vec![],
        pre_dispatch_policies: vec![],
        post_turn_policies: vec![],
        post_loop_policies: vec![],
        async_transform_context: None,
        metrics_collector: None,
        fallback: None,
        tool_execution_policy: swink_agent::ToolExecutionPolicy::default(),
        session_state: Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
        credential_resolver: None,
        cache_config: None,
        cache_state: std::sync::Mutex::new(swink_agent::CacheState::default()),
        dynamic_system_prompt: None,
    }
}

async fn collect_events(stream: Pin<Box<dyn Stream<Item = AgentEvent> + Send>>) -> Vec<AgentEvent> {
    stream.collect().await
}

fn user_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        timestamp: 0,
        cache_hint: None,
    }))
}

/// The canned truncation sequence from issue #619: a tool-use block starts
/// streaming, the provider hits `max_tokens` mid-JSON, and emits `Done(Length)`
/// without a matching `ToolCallEnd`.
fn truncated_tool_call_events() -> Vec<AssistantMessageEvent> {
    vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            id: "tc_truncated".into(),
            name: "echo".into(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            delta: r#"{"msg": "hel"#.into(),
        },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Length,
            usage: Usage::default(),
            cost: Cost::default(),
        },
    ]
}

/// After the first truncated turn, the loop inserts a synthetic error tool
/// result and runs a second turn. The follow-up turn just returns plain text.
fn plain_text_done_events(text: &str) -> Vec<AssistantMessageEvent> {
    vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::TextStart { content_index: 0 },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: text.to_string(),
        },
        AssistantMessageEvent::TextEnd { content_index: 0 },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            usage: Usage::default(),
            cost: Cost::default(),
        },
    ]
}

/// End-to-end: feed the canned truncation stream on turn 1 and a plain-text
/// response on turn 2. Verify that the assistant message in the final
/// `AgentEnd.messages` has its truncated tool-call block scrubbed: arguments
/// is an empty object and `partial_json` is `None`.
///
/// Without the fix, the tool-use block in the history would carry
/// `arguments: Null` + `partial_json: Some(..)`, which adapters forward
/// verbatim and providers reject with HTTP 400 on the second turn.
#[tokio::test]
async fn loop_sanitizes_truncated_tool_call_before_adapter_replay() {
    let stream = Arc::new(MockStreamFn::new(vec![
        truncated_tool_call_events(),
        plain_text_done_events("done"),
    ]));
    let tool: Arc<dyn AgentTool> = Arc::new(EchoTool);
    let config = loop_config(stream, tool);
    let cancel = CancellationToken::new();

    let events = collect_events(agent_loop(
        vec![user_msg("do thing")],
        "sys".to_string(),
        config,
        cancel,
    ))
    .await;

    // Grab the final history off AgentEnd.
    let final_messages = events
        .iter()
        .rev()
        .find_map(|e| match e {
            AgentEvent::AgentEnd { messages } => Some(messages.clone()),
            _ => None,
        })
        .expect("expected AgentEnd event");

    // Collect every ToolCall block the adapter would see on replay.
    let tool_call_blocks: Vec<&ContentBlock> = final_messages
        .iter()
        .filter_map(|m| match m {
            AgentMessage::Llm(LlmMessage::Assistant(a)) => Some(a),
            _ => None,
        })
        .flat_map(|a| a.content.iter())
        .filter(|b| matches!(b, ContentBlock::ToolCall { .. }))
        .collect();

    assert!(
        !tool_call_blocks.is_empty(),
        "expected at least one tool_call block in committed history"
    );

    for block in &tool_call_blocks {
        match block {
            ContentBlock::ToolCall {
                arguments,
                partial_json,
                ..
            } => {
                assert!(
                    arguments.is_object(),
                    "adapter replay invariant violated: arguments must be a JSON object, got {arguments:?}"
                );
                assert_ne!(
                    *arguments,
                    Value::Null,
                    "adapter replay invariant violated: arguments is Null"
                );
                assert!(
                    partial_json.is_none(),
                    "adapter replay invariant violated: partial_json should be cleared, got {partial_json:?}"
                );
            }
            _ => unreachable!(),
        }
    }
}
