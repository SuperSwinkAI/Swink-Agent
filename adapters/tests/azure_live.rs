//! Live API tests for `AzureStreamFn`.

use std::time::Duration;

use futures::StreamExt;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use swink_agent::{AgentContext, AssistantMessageEvent, ModelSpec, StreamFn, StreamOptions};
use swink_agent_adapters::AzureStreamFn;

const TIMEOUT: Duration = Duration::from_secs(30);

fn stream_fn() -> AzureStreamFn {
    dotenvy::dotenv().ok();
    AzureStreamFn::new(
        std::env::var("AZURE_BASE_URL").expect("AZURE_BASE_URL must be set"),
        std::env::var("AZURE_API_KEY").expect("AZURE_API_KEY must be set"),
    )
}

#[tokio::test]
#[ignore = "hits live API"]
async fn live_text_stream() {
    let stream_fn = stream_fn();
    let model = ModelSpec::new(
        "azure",
        std::env::var("AZURE_MODEL").unwrap_or_else(|_| "gpt-5.4".into()),
    );
    let context = AgentContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let options = StreamOptions::default();
    let stream = stream_fn.stream(&model, &context, &options, CancellationToken::new());
    let events = timeout(TIMEOUT, stream.collect::<Vec<AssistantMessageEvent>>())
        .await
        .unwrap();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::Done { .. }))
    );
}
