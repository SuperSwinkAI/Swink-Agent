//! xAI Grok adapter.
//!
//! xAI currently exposes a chat-completions-compatible API surface for text and
//! tool calling, so this adapter delegates transport details to the existing
//! OpenAI-compatible implementation while keeping xAI as a first-class type.

use std::pin::Pin;

use futures::Stream;
use tokio_util::sync::CancellationToken;

use swink_agent::{AgentContext, AssistantMessageEvent, ModelSpec, StreamFn, StreamOptions};

use crate::oai_transport::OaiAdapterShell;

pub struct XAiStreamFn {
    shell: OaiAdapterShell,
}

impl XAiStreamFn {
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            shell: OaiAdapterShell::new("xAI", base_url, api_key),
        }
    }
}

impl std::fmt::Debug for XAiStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.shell.fmt_debug("XAiStreamFn", f)
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
        self.shell
            .stream(model, context, options, cancellation_token)
    }
}
