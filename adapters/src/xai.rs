//! xAI Grok adapter.
//!
//! xAI currently exposes a chat-completions-compatible API surface for text and
//! tool calling, so this adapter delegates transport details to the existing
//! OpenAI-compatible implementation while keeping xAI as a first-class type.

use std::pin::Pin;

use futures::Stream;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use swink_agent::{AgentContext, AssistantMessageEvent, ModelSpec, StreamFn, StreamOptions};

use crate::base::AdapterBase;
use crate::oai_transport::{oai_send_and_parse, prepare_oai_request};

pub struct XAiStreamFn {
    base: AdapterBase,
}

impl XAiStreamFn {
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base: AdapterBase::new(base_url, api_key),
        }
    }
}

impl std::fmt::Debug for XAiStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XAiStreamFn")
            .field("base_url", &self.base.base_url)
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}

impl StreamFn for XAiStreamFn {
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        let url = format!("{}/v1/chat/completions", self.base.base_url);
        let api_key = options.api_key.as_deref().unwrap_or(&self.base.api_key);

        debug!(
            %url,
            model = %model.model_id,
            messages = context.messages.len(),
            "sending xAI request"
        );

        let request = prepare_oai_request(&self.base.client, &url, model, context, options)
            .header("Authorization", format!("Bearer {api_key}"));

        Box::pin(oai_send_and_parse(
            request,
            "xAI",
            cancellation_token,
            |_, _| None,
        ))
    }
}
