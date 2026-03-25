#![cfg(feature = "azure")]
//! Wiremock-based tests for `AzureStreamFn`.

mod common;

use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer};

use swink_agent::{AssistantMessageEvent, ModelSpec, StopReason, StreamFn, StreamOptions};
use swink_agent_adapters::AzureStreamFn;

use common::{sse_response, test_context};

fn test_model() -> ModelSpec {
    ModelSpec::new("azure", "gpt-5.4")
}

async fn collect_events(stream_fn: &AzureStreamFn) -> Vec<AssistantMessageEvent> {
    let token = CancellationToken::new();
    stream_fn
        .stream(
            &test_model(),
            &test_context(),
            &StreamOptions::default(),
            token,
        )
        .collect::<Vec<_>>()
        .await
}

#[tokio::test]
async fn azure_text_stream() {
    let body = [
        r#"data: {"choices":[{"delta":{"content":"hello"}}]}"#,
        "",
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":2}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/openai/v1/chat/completions"))
        .and(header("api-key", "test-key"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let stream_fn = AzureStreamFn::new(format!("{}/openai/v1", server.uri()), "test-key");
    let events = collect_events(&stream_fn).await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::Start))
    );
    assert!(
        events.iter().any(
            |e| matches!(e, AssistantMessageEvent::TextDelta { delta, .. } if delta == "hello")
        )
    );
    assert!(events.iter().any(|e| matches!(
        e,
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            ..
        }
    )));
}
