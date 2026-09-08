//! OpenAI adapter.
//!
//! [`OpenAiStreamFn::new`] speaks the **Responses API** (`/v1/responses`),
//! OpenAI's primary surface for its current models: `reasoning.effort`,
//! reasoning summaries and richer prompt-cache accounting exist only there.
//! It is a thin wrapper over the shared [`ResponsesStreamFn`] shell.
//!
//! [`OpenAiStreamFn::new_chat_completions`] speaks **Chat Completions**
//! (`/v1/chat/completions`) over the shared [`OaiAdapterShell`]. It is not
//! deprecated: it is the path for OpenAI-*compatible* third-party endpoints
//! — vLLM, LM Studio, Groq, Together and friends implement Chat Completions,
//! not Responses. Pick by what the endpoint speaks, not by preference.
//!
//! Both produce the same [`AssistantMessageEvent`] stream; nothing downstream
//! of [`StreamFn`] can tell them apart.
//!
//! Hosts that build adapters from configuration rather than code (the preset
//! factory, the TUI) pick the wire with [`OpenAiWire::from_env`]: the
//! `OPENAI_API` environment variable, `responses` (default) or
//! `chat_completions`. That is the knob for an `OPENAI_BASE_URL` that points
//! at an OpenAI-compatible server.

use std::pin::Pin;

use futures::Stream;
use tokio_util::sync::CancellationToken;

use swink_agent::{
    AgentContext, AssistantMessageEvent, ModelSpec, ServingOptionSupport, StreamFn, StreamOptions,
};

use crate::oai_transport::OaiAdapterShell;
use crate::responses::ResponsesStreamFn;

// ─── Wire selection ─────────────────────────────────────────────────────────

/// Environment variable read by [`OpenAiWire::from_env`].
pub const OPENAI_API_ENV: &str = "OPENAI_API";

/// Which OpenAI wire protocol an [`OpenAiStreamFn`] speaks.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenAiWire {
    /// `/v1/responses` — OpenAI proper. The default.
    #[default]
    Responses,
    /// `/v1/chat/completions` — OpenAI-compatible third-party servers.
    ChatCompletions,
}

/// `OPENAI_API` held a value other than `responses` / `chat_completions`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "{OPENAI_API_ENV}={value:?} is not a wire protocol; expected \"responses\" or \"chat_completions\""
)]
pub struct InvalidOpenAiWire {
    /// The offending value.
    pub value: String,
}

impl OpenAiWire {
    /// Parse a wire name: `responses` or `chat_completions` (case-insensitive,
    /// `-` accepted for `_`).
    ///
    /// # Errors
    /// [`InvalidOpenAiWire`] for anything else.
    pub fn parse(value: &str) -> Result<Self, InvalidOpenAiWire> {
        match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "responses" => Ok(Self::Responses),
            "chat_completions" => Ok(Self::ChatCompletions),
            _ => Err(InvalidOpenAiWire {
                value: value.to_owned(),
            }),
        }
    }

    /// The wire named by `OPENAI_API`, or [`OpenAiWire::Responses`] when it
    /// is unset or empty. An unrecognised value is an error, never a silent
    /// default: a misspelt knob must not quietly route to the wrong protocol.
    ///
    /// The environment read is process-global; a host that runs several
    /// `openai` connections with different base URLs must pick the wire per
    /// connection and pass it to [`OpenAiStreamFn::new_for_wire`] instead.
    ///
    /// # Errors
    /// [`InvalidOpenAiWire`] when the variable is set to an unknown value.
    pub fn from_env() -> Result<Self, InvalidOpenAiWire> {
        match std::env::var(OPENAI_API_ENV) {
            Ok(value) if !value.trim().is_empty() => Self::parse(&value),
            _ => Ok(Self::Responses),
        }
    }
}

// ─── OpenAiStreamFn ─────────────────────────────────────────────────────────

enum Backend {
    Responses(ResponsesStreamFn),
    ChatCompletions(OaiAdapterShell),
}

/// A [`StreamFn`] for OpenAI — Responses API by default, Chat Completions
/// for OpenAI-compatible endpoints. See the `openai` module documentation.
pub struct OpenAiStreamFn {
    backend: Backend,
}

impl OpenAiStreamFn {
    /// OpenAI proper, over the Responses API.
    ///
    /// # Arguments
    ///
    /// * `base_url` - API base URL (e.g. `https://api.openai.com`).
    /// * `api_key` - Bearer token for authentication.
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            backend: Backend::Responses(
                ResponsesStreamFn::new(base_url, api_key).with_provider_label("OpenAI"),
            ),
        }
    }

    /// An OpenAI-compatible endpoint, over Chat Completions.
    ///
    /// Use this for vLLM, LM Studio, Groq, Together, or any other server
    /// that implements `/v1/chat/completions` but not `/v1/responses`.
    #[must_use]
    pub fn new_chat_completions(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            backend: Backend::ChatCompletions(OaiAdapterShell::new("OpenAI", base_url, api_key)),
        }
    }

    /// Construct for an explicit [`OpenAiWire`] — what configuration-driven
    /// hosts call after [`OpenAiWire::from_env`].
    #[must_use]
    pub fn new_for_wire(
        wire: OpenAiWire,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        match wire {
            OpenAiWire::ChatCompletions => Self::new_chat_completions(base_url, api_key),
            // `Responses`, and any future variant: the default wire.
            _ => Self::new(base_url, api_key),
        }
    }

    /// `true` when this adapter speaks the Responses API.
    #[must_use]
    pub const fn uses_responses_api(&self) -> bool {
        matches!(self.backend, Backend::Responses(_))
    }

    /// Add one static header to every request.
    ///
    /// Useful for org-scoped accounts (`OpenAI-Organization`,
    /// `OpenAI-Project`) and for gateway deployments that demand their own
    /// headers. Supplying `Authorization` here replaces the default
    /// `Bearer` header.
    #[must_use]
    pub fn with_header(
        mut self,
        name: reqwest::header::HeaderName,
        value: reqwest::header::HeaderValue,
    ) -> Self {
        self.backend = match self.backend {
            Backend::Responses(inner) => Backend::Responses(inner.with_header(name, value)),
            Backend::ChatCompletions(shell) => {
                Backend::ChatCompletions(shell.with_header(name, value))
            }
        };
        self
    }

    /// Merge a header map into every request, replacing colliding names.
    #[must_use]
    pub fn with_headers(mut self, headers: reqwest::header::HeaderMap) -> Self {
        self.backend = match self.backend {
            Backend::Responses(inner) => Backend::Responses(inner.with_headers(headers)),
            Backend::ChatCompletions(shell) => {
                Backend::ChatCompletions(shell.with_headers(headers))
            }
        };
        self
    }

    #[cfg(test)]
    fn base_url(&self) -> &str {
        match &self.backend {
            Backend::Responses(inner) => inner.shell.base_url(),
            Backend::ChatCompletions(shell) => shell.base_url(),
        }
    }
}

impl std::fmt::Debug for OpenAiStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.backend {
            Backend::Responses(inner) => inner.shell.fmt_debug("OpenAiStreamFn(responses)", f),
            Backend::ChatCompletions(shell) => {
                shell.fmt_debug("OpenAiStreamFn(chat_completions)", f)
            }
        }
    }
}

impl StreamFn for OpenAiStreamFn {
    fn supported_serving_options(&self) -> ServingOptionSupport {
        match &self.backend {
            Backend::Responses(inner) => inner.supported_serving_options(),
            // Chat Completions: `top_p`, `format` and `extra` reach the body;
            // `context_length` / `keep_alive` / `reasoning_effort` have no
            // equivalent on that protocol.
            Backend::ChatCompletions(_) => ServingOptionSupport::none()
                .with_top_p(true)
                .with_format(true)
                .with_extra(true),
        }
    }

    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        match &self.backend {
            Backend::Responses(inner) => inner.stream(model, context, options, cancellation_token),
            Backend::ChatCompletions(shell) => {
                shell.stream(model, context, options, cancellation_token)
            }
        }
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
    fn trailing_slash_stripped_on_both_backends() {
        let responses = OpenAiStreamFn::new("https://api.openai.com/", "key");
        assert_eq!(responses.base_url(), "https://api.openai.com");
        assert!(responses.uses_responses_api());

        let chat = OpenAiStreamFn::new_chat_completions("https://api.openai.com/", "key");
        assert_eq!(chat.base_url(), "https://api.openai.com");
        assert!(!chat.uses_responses_api());
    }

    #[test]
    fn debug_names_the_backend_and_redacts_the_key() {
        let rendered = format!(
            "{:?}",
            OpenAiStreamFn::new("https://api.openai.com", "sk-secret")
        );
        assert!(rendered.contains("responses"), "{rendered}");
        assert!(!rendered.contains("sk-secret"), "{rendered}");
        let rendered = format!(
            "{:?}",
            OpenAiStreamFn::new_chat_completions("https://api.openai.com", "sk-secret")
        );
        assert!(rendered.contains("chat_completions"), "{rendered}");
        assert!(!rendered.contains("sk-secret"), "{rendered}");
    }

    #[test]
    fn wire_parses_both_names_and_rejects_the_rest() {
        assert_eq!(OpenAiWire::parse("responses"), Ok(OpenAiWire::Responses));
        assert_eq!(
            OpenAiWire::parse("Chat-Completions "),
            Ok(OpenAiWire::ChatCompletions)
        );
        assert_eq!(
            OpenAiWire::parse("chat_completions"),
            Ok(OpenAiWire::ChatCompletions)
        );
        let err = OpenAiWire::parse("completions").unwrap_err();
        assert_eq!(err.value, "completions");
        assert!(err.to_string().contains("OPENAI_API"), "{err}");
    }

    #[test]
    fn from_env_defaults_to_responses_when_unset() {
        // Read-only: this workspace forbids unsafe code, and set_var is unsafe
        // as of edition 2024, so no test may mutate OPENAI_API. Skip rather
        // than assert if a developer's shell happens to export it.
        if std::env::var_os(OPENAI_API_ENV).is_none() {
            assert_eq!(OpenAiWire::from_env(), Ok(OpenAiWire::Responses));
        }
    }

    #[test]
    fn new_for_wire_selects_the_backend() {
        assert!(OpenAiWire::default() == OpenAiWire::Responses);
        assert!(OpenAiStreamFn::new_for_wire(OpenAiWire::Responses, "u", "k").uses_responses_api());
        assert!(
            !OpenAiStreamFn::new_for_wire(OpenAiWire::ChatCompletions, "u", "k")
                .uses_responses_api()
        );
    }

    #[test]
    fn serving_support_differs_by_backend() {
        assert!(
            OpenAiStreamFn::new("u", "k")
                .supported_serving_options()
                .reasoning_effort
        );
        assert!(
            !OpenAiStreamFn::new_chat_completions("u", "k")
                .supported_serving_options()
                .reasoning_effort
        );
    }
}
