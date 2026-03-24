//! OpenAI-compatible LLM adapter.
//!
//! Implements [`StreamFn`] for any OpenAI-compatible chat completions API
//! (OpenAI, vLLM, LM Studio, Groq, Together, etc.). These all share the
//! same SSE streaming format.

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

// ─── OpenAiStreamFn ─────────────────────────────────────────────────────────

/// A [`StreamFn`] implementation for OpenAI-compatible chat completions APIs.
///
/// Works with OpenAI, vLLM, LM Studio, Groq, Together, and any other provider
/// that implements the OpenAI chat completions SSE streaming format.
pub struct OpenAiStreamFn {
    pub(crate) base: AdapterBase,
}

impl OpenAiStreamFn {
    /// Create a new OpenAI-compatible stream function.
    ///
    /// # Arguments
    ///
    /// * `base_url` - API base URL (e.g. `https://api.openai.com`).
    /// * `api_key` - Bearer token for authentication.
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base: AdapterBase::new(base_url, api_key),
        }
    }
}

impl std::fmt::Debug for OpenAiStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiStreamFn")
            .field("base_url", &self.base.base_url)
            .field("api_key", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl StreamFn for OpenAiStreamFn {
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        Box::pin(openai_stream(
            self,
            model,
            context,
            options,
            cancellation_token,
        ))
    }
}

// ─── Stream implementation ──────────────────────────────────────────────────

fn openai_stream<'a>(
    openai: &'a OpenAiStreamFn,
    model: &'a ModelSpec,
    context: &'a AgentContext,
    options: &'a StreamOptions,
    cancellation_token: CancellationToken,
) -> impl Stream<Item = AssistantMessageEvent> + Send + 'a {
    stream::once(async move {
        let response = match send_request(openai, model, context, options).await {
            Ok(resp) => resp,
            Err(event) => return stream::iter(vec![event]).left_stream(),
        };

        let status = response.status();
        if !status.is_success() {
            let code = status.as_u16();
            let body = response.text().await.unwrap_or_default();
            warn!(status = code, "OpenAI HTTP error");
            let event = crate::classify::error_event_from_status(code, &body, "OpenAI");
            return stream::iter(vec![event]).left_stream();
        }

        parse_oai_sse_stream(response, cancellation_token, "OpenAI").right_stream()
    })
    .flatten()
}

/// Send the HTTP POST request to the OpenAI-compatible endpoint.
async fn send_request(
    openai: &OpenAiStreamFn,
    model: &ModelSpec,
    context: &AgentContext,
    options: &StreamOptions,
) -> Result<reqwest::Response, AssistantMessageEvent> {
    let url = format!("{}/v1/chat/completions", openai.base.base_url);
    debug!(
        %url,
        model = %model.model_id,
        messages = context.messages.len(),
        "sending OpenAI request"
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

    let api_key = options.api_key.as_deref().unwrap_or(&openai.base.api_key);

    openai
        .base
        .client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| AssistantMessageEvent::error_network(format!("OpenAI connection error: {e}")))
}

// ─── Compile-time assertions ────────────────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<OpenAiStreamFn>();
};
