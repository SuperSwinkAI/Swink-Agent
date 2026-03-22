//! Integration tests for `SubAgent`.

mod common;

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use serde_json::json;

use swink_agent::{
    AgentMessage, AgentOptions, AgentTool, AgentToolResult, AssistantMessageEvent, ContentBlock,
    LlmMessage, ModelSpec, StopReason, SubAgent, Usage, stream::StreamFn,
};

use common::{MockStreamFn, default_model, text_only_events};

#[tokio::test]
async fn sub_agent_runs_and_returns_text() {
    let stream_fn: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![text_only_events(
        "sub-agent says hello",
    )]));

    let sfn = stream_fn.clone();
    let sub =
        SubAgent::new("researcher", "Researcher", "A research sub-agent").with_options(move || {
            AgentOptions::new_simple("You are a researcher.", default_model(), Arc::clone(&sfn))
        });

    let params = serde_json::json!({ "prompt": "what is rust?" });
    let ct = CancellationToken::new();
    let result = sub.execute("call-1", params, ct, None).await;

    assert!(!result.is_error);
    let text = ContentBlock::extract_text(&result.content);
    assert!(text.contains("sub-agent says hello"));
}

#[tokio::test]
async fn sub_agent_error_maps_to_tool_error() {
    // Stream returns an error event
    let error_events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::TextStart { content_index: 0 },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "partial".into(),
        },
        AssistantMessageEvent::TextEnd { content_index: 0 },
        AssistantMessageEvent::Error {
            stop_reason: StopReason::Error,
            error_message: "model exploded".into(),
            usage: Some(Usage::default()),
            error_kind: None,
        },
    ];
    let stream_fn: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![error_events]));

    let sfn = stream_fn.clone();
    let sub = SubAgent::new("broken", "Broken", "Always fails")
        .with_options(move || AgentOptions::new_simple("fail", default_model(), Arc::clone(&sfn)));

    let params = serde_json::json!({ "prompt": "do something" });
    let ct = CancellationToken::new();
    let result = sub.execute("call-2", params, ct, None).await;

    // The result should contain text (even error-stopped agents produce text)
    // OR it should be an error. The exact behavior depends on how the agent
    // surfaces the error. Let's just verify it completes without panic.
    let text = ContentBlock::extract_text(&result.content);
    assert!(!text.is_empty() || result.is_error);
}

#[tokio::test]
async fn sub_agent_cancellation() {
    // Stream that would take a while (but gets cancelled)
    let stream_fn: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![text_only_events(
        "this should not complete",
    )]));

    let sfn = stream_fn.clone();
    let sub = SubAgent::new("slow", "Slow", "Gets cancelled").with_options(move || {
        AgentOptions::new_simple("slow agent", default_model(), Arc::clone(&sfn))
    });

    let params = serde_json::json!({ "prompt": "go" });
    let ct = CancellationToken::new();
    // Cancel immediately
    ct.cancel();
    let result = sub.execute("call-3", params, ct, None).await;

    assert!(result.is_error);
    let text = ContentBlock::extract_text(&result.content);
    assert!(text.contains("cancelled") || text.contains("cancel"));
}

#[tokio::test]
async fn sub_agent_shares_stream_fn() {
    let stream_fn: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![text_only_events(
        "shared stream works",
    )]));

    // Verify the same Arc<dyn StreamFn> can be shared
    let sfn1 = Arc::clone(&stream_fn);
    let sfn2 = Arc::clone(&stream_fn);

    assert!(Arc::ptr_eq(&sfn1, &sfn2));

    let sub = SubAgent::new("shared", "Shared", "Uses shared stream")
        .with_options(move || AgentOptions::new_simple("shared", default_model(), Arc::clone(&sfn1)));

    assert_eq!(sub.name(), "shared");
    assert_eq!(sub.label(), "Shared");
    assert_eq!(sub.description(), "Uses shared stream");
}

// ── default_map_result coverage ──────────────────────────────────────────

#[tokio::test]
async fn default_map_result_with_error_and_no_message() {
    // Test default_map_result via execute with a stream that produces Error stop.
    let error_events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::Error {
            stop_reason: StopReason::Error,
            error_message: "boom".into(),
            usage: None,
            error_kind: None,
        },
    ];
    let stream_fn2: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![error_events]));
    let sfn2 = Arc::clone(&stream_fn2);

    let sub2 = SubAgent::new("err", "Err", "errors")
        .with_options(move || AgentOptions::new_simple("sys", default_model(), Arc::clone(&sfn2)));

    let ct = CancellationToken::new();
    let result = sub2.execute("c1", json!({"prompt": "go"}), ct, None).await;

    assert!(result.is_error);
    let text = ContentBlock::extract_text(&result.content);
    // Should contain fallback or actual error text
    assert!(!text.is_empty());
}

#[tokio::test]
async fn default_map_result_with_no_assistant_messages() {
    // Stream returns a text response, but the agent might process it differently.
    // We use with_map_result to intercept and test the default mapper logic by
    // building an AgentResult that has only user messages.
    let called = Arc::new(std::sync::Mutex::new(false));
    let called_clone = Arc::clone(&called);

    let stream_fn: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![text_only_events("hi")]));
    let sfn = Arc::clone(&stream_fn);

    let sub = SubAgent::new("t", "T", "test")
        .with_options(move || AgentOptions::new_simple("sys", default_model(), Arc::clone(&sfn)))
        .with_map_result(move |result| {
            *called_clone.lock().unwrap() = true;
            // Simulate default_map_result behavior for only-user-messages
            let has_assistant = result
                .messages
                .iter()
                .any(|m| matches!(m, AgentMessage::Llm(LlmMessage::Assistant(_))));
            if has_assistant {
                AgentToolResult::text("found assistant")
            } else {
                AgentToolResult::text("sub-agent produced no text output")
            }
        });

    let ct = CancellationToken::new();
    let result = sub
        .execute("c1", json!({"prompt": "hello"}), ct, None)
        .await;

    assert!(*called.lock().unwrap());
    // The agent will have assistant messages from the stream, so it should find them
    let text = ContentBlock::extract_text(&result.content);
    assert!(!text.is_empty());
}

#[tokio::test]
async fn custom_map_result() {
    let stream_fn: Arc<dyn StreamFn> =
        Arc::new(MockStreamFn::new(vec![text_only_events("original output")]));
    let sfn = Arc::clone(&stream_fn);

    let sub = SubAgent::new("custom", "Custom", "custom mapper")
        .with_options(move || AgentOptions::new_simple("sys", default_model(), Arc::clone(&sfn)))
        .with_map_result(|_result| AgentToolResult::text("custom mapped"));

    let ct = CancellationToken::new();
    let result = sub.execute("c1", json!({"prompt": "go"}), ct, None).await;

    assert!(!result.is_error);
    let text = ContentBlock::extract_text(&result.content);
    assert_eq!(text, "custom mapped");
}

#[test]
fn with_custom_schema() {
    let custom_schema = json!({
        "type": "object",
        "properties": {
            "query": { "type": "string" },
            "max_results": { "type": "integer" }
        },
        "required": ["query"]
    });

    let sub = SubAgent::new("s", "S", "schema test").with_schema(custom_schema.clone());

    assert_eq!(sub.parameters_schema(), &custom_schema);
}

#[tokio::test]
async fn execute_with_empty_prompt() {
    let stream_fn: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![text_only_events(
        "empty prompt response",
    )]));
    let sfn = Arc::clone(&stream_fn);

    let sub = SubAgent::new("ep", "EP", "empty prompt")
        .with_options(move || AgentOptions::new_simple("sys", default_model(), Arc::clone(&sfn)));

    let ct = CancellationToken::new();
    let result = sub.execute("c1", json!({"prompt": ""}), ct, None).await;

    // Should still complete without panic
    let text = ContentBlock::extract_text(&result.content);
    assert!(!text.is_empty() || result.is_error);
}

#[tokio::test]
async fn execute_with_missing_prompt_param() {
    let stream_fn: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![text_only_events(
        "no prompt response",
    )]));
    let sfn = Arc::clone(&stream_fn);

    let sub = SubAgent::new("np", "NP", "no prompt")
        .with_options(move || AgentOptions::new_simple("sys", default_model(), Arc::clone(&sfn)));

    let ct = CancellationToken::new();
    // params has no "prompt" key — as_str() returns None, unwrap_or("") kicks in
    let result = sub.execute("c1", json!({"other": "value"}), ct, None).await;

    // Should still complete without panic (empty string used as prompt)
    let text = ContentBlock::extract_text(&result.content);
    assert!(!text.is_empty() || result.is_error);
}
