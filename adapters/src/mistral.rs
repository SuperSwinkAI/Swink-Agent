//! Native Mistral adapter.
//!
//! Mistral's text and tool-calling APIs are currently chat-completions
//! compatible, so this adapter reuses the OpenAI-compatible transport while
//! keeping Mistral-specific presets and typing explicit.

use std::pin::Pin;

use futures::Stream;
use tokio_util::sync::CancellationToken;

use swink_agent::stream::{AssistantMessageEvent, StreamFn, StreamOptions};
use swink_agent::types::{AgentContext, ModelSpec};

use crate::OpenAiStreamFn;

pub struct MistralStreamFn {
    inner: OpenAiStreamFn,
}

impl MistralStreamFn {
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            inner: OpenAiStreamFn::new(base_url, api_key),
        }
    }
}

impl std::fmt::Debug for MistralStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MistralStreamFn")
            .field("inner", &self.inner)
            .finish()
    }
}

impl StreamFn for MistralStreamFn {
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        self.inner
            .stream(model, context, options, cancellation_token)
    }
}
