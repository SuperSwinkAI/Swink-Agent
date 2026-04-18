//! OpenAI-compatible LLM adapter.
//!
//! Implements [`StreamFn`] for any OpenAI-compatible chat completions API
//! (OpenAI, vLLM, LM Studio, Groq, Together, etc.). These all share the
//! same SSE streaming format.

use std::pin::Pin;

use futures::Stream;
use tokio_util::sync::CancellationToken;

use swink_agent::{AgentContext, AssistantMessageEvent, ModelSpec, StreamFn, StreamOptions};

use crate::oai_transport::OaiAdapterShell;

// ─── OpenAiStreamFn ─────────────────────────────────────────────────────────

/// A [`StreamFn`] implementation for OpenAI-compatible chat completions APIs.
///
/// Works with OpenAI, vLLM, LM Studio, Groq, Together, and any other provider
/// that implements the OpenAI chat completions SSE streaming format.
pub struct OpenAiStreamFn {
    pub(crate) shell: OaiAdapterShell,
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
            shell: OaiAdapterShell::new("OpenAI", base_url, api_key),
        }
    }
}

impl std::fmt::Debug for OpenAiStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.shell.fmt_debug("OpenAiStreamFn", f)
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
        self.shell
            .stream(model, context, options, cancellation_token)
    }
}

// ─── Compile-time assertions ────────────────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<OpenAiStreamFn>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailing_slash_stripped() {
        let oai = OpenAiStreamFn::new("https://api.openai.com/", "key");
        assert_eq!(oai.shell.base_url(), "https://api.openai.com");
    }

    #[test]
    fn no_trailing_slash_unchanged() {
        let oai = OpenAiStreamFn::new("https://api.openai.com", "key");
        assert_eq!(oai.shell.base_url(), "https://api.openai.com");
    }
}
