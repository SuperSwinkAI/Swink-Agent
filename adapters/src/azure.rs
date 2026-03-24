//! Azure `OpenAI` / Azure AI Foundry adapter.
//!
//! This adapter targets Azure's OpenAI-v1-compatible chat completions surface.
//! It reuses the shared OAI-compatible SSE parsing from [`openai_compat`].

use std::pin::Pin;

use futures::stream::{self, Stream, StreamExt as _};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use swink_agent::stream::{AssistantMessageEvent, StreamFn, StreamOptions};
use swink_agent::types::{AgentContext, ModelSpec};

use crate::base::AdapterBase;
use crate::convert;
use crate::openai_compat::{
    OaiChatRequest, OaiConverter, OaiStreamOptions, build_oai_tools, parse_oai_sse_stream,
};

pub struct AzureStreamFn {
    base: AdapterBase,
}

impl AzureStreamFn {
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base: AdapterBase::new(base_url.into().trim_end_matches('/').to_string(), api_key),
        }
    }
}

impl std::fmt::Debug for AzureStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AzureStreamFn")
            .field("base_url", &self.base.base_url)
            .field("api_key", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl StreamFn for AzureStreamFn {
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        Box::pin(azure_stream(
            self,
            model,
            context,
            options,
            cancellation_token,
        ))
    }
}

fn azure_stream<'a>(
    azure: &'a AzureStreamFn,
    model: &'a ModelSpec,
    context: &'a AgentContext,
    options: &'a StreamOptions,
    cancellation_token: CancellationToken,
) -> impl Stream<Item = AssistantMessageEvent> + Send + 'a {
    stream::once(async move {
        let response = match send_request(azure, model, context, options).await {
            Ok(resp) => resp,
            Err(event) => return stream::iter(vec![event]).left_stream(),
        };

        let status = response.status();
        if !status.is_success() {
            let code = status.as_u16();
            let body = response.text().await.unwrap_or_default();
            warn!(status = code, "Azure HTTP error");
            let event = crate::classify::error_event_from_status(code, &body, "Azure");
            return stream::iter(vec![event]).left_stream();
        }

        parse_oai_sse_stream(response, cancellation_token, "Azure").right_stream()
    })
    .flatten()
}

async fn send_request(
    azure: &AzureStreamFn,
    model: &ModelSpec,
    context: &AgentContext,
    options: &StreamOptions,
) -> Result<reqwest::Response, AssistantMessageEvent> {
    let url = format!("{}/chat/completions", azure.base.base_url);
    debug!(
        %url,
        model = %model.model_id,
        messages = context.messages.len(),
        "sending Azure request"
    );

    let messages =
        convert::convert_messages::<OaiConverter>(&context.messages, &context.system_prompt);

    let (tools, tool_choice) = build_oai_tools(&context.tools);

    let body = OaiChatRequest {
        model: model.model_id.clone(),
        messages,
        stream: true,
        stream_options: OaiStreamOptions {
            include_usage: true,
        },
        temperature: options.temperature,
        max_tokens: options.max_tokens,
        tools,
        tool_choice,
    };

    let api_key = options.api_key.as_deref().unwrap_or(&azure.base.api_key);

    azure
        .base
        .client
        .post(&url)
        .header("api-key", api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| AssistantMessageEvent::error_network(format!("Azure connection error: {e}")))
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AzureStreamFn>();
};
