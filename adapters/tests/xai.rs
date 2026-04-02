#![cfg(feature = "xai")]
//! Wiremock-based tests for `XAiStreamFn`.

use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use swink_agent::{AgentContext, AssistantMessageEvent, ModelSpec, StreamFn, StreamOptions};
use swink_agent_adapters::XAiStreamFn;

#[tokio::test]
async fn xai_wrapper_streams_chat_completions() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"ok"}}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let stream_fn = XAiStreamFn::new(server.uri(), "test-key");
    let events = stream_fn
        .stream(
            &ModelSpec::new("xai", "grok-4-1-fast-non-reasoning"),
            &AgentContext {
                system_prompt: String::new(),
                messages: Vec::new(),
                tools: Vec::new(),
            },
            &StreamOptions::default(),
            CancellationToken::new(),
        )
        .collect::<Vec<AssistantMessageEvent>>()
        .await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::TextDelta { delta, .. } if delta == "ok"))
    );
}
