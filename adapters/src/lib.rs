#![forbid(unsafe_code)]
//! LLM provider adapters for [`swink-agent`](https://docs.rs/swink-agent).
//!
//! Provides [`StreamFn`](swink_agent::StreamFn) implementations for nine LLM providers.
//! No provider is enabled by default; enable only what you need:
//!
//! | Feature | Provider |
//! |---|---|
//! | `anthropic` | Anthropic Claude |
//! | `openai` | `OpenAI` GPT |
//! | `ollama` | `Ollama` (local) |
//! | `gemini` | Google Gemini |
//! | `azure` | Azure `OpenAI` / AI Foundry |
//! | `bedrock` | AWS Bedrock |
//! | `mistral` | Mistral AI |
//! | `xai` | xAI Grok |
//! | `proxy` | Custom SSE proxy |
//!
//! Use `full` or `all` to compile every provider adapter.
//!
//! # Quick Start
//!
//! ```no_run
//! use swink_agent_adapters::build_remote_connection_for_model;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let conn = build_remote_connection_for_model("claude-sonnet-4-6")?;
//! # Ok(())
//! # }
//! ```

// ── Shared infrastructure (always compiled) ───────────────────────────────
#[cfg_attr(
    not(any(
        feature = "anthropic",
        feature = "openai",
        feature = "ollama",
        feature = "gemini",
        feature = "proxy",
        feature = "azure",
        feature = "bedrock",
        feature = "mistral",
        feature = "xai",
    )),
    allow(dead_code)
)]
mod base;
#[cfg_attr(
    not(any(
        feature = "anthropic",
        feature = "openai",
        feature = "ollama",
        feature = "gemini",
        feature = "proxy",
        feature = "azure",
        feature = "bedrock",
        feature = "mistral",
        feature = "xai",
    )),
    allow(dead_code)
)]
mod block_accumulator;
pub mod classify;
pub mod convert;
#[cfg_attr(
    not(any(
        feature = "anthropic",
        feature = "openai",
        feature = "ollama",
        feature = "gemini",
        feature = "proxy",
        feature = "azure",
        feature = "bedrock",
        feature = "mistral",
        feature = "xai",
    )),
    allow(dead_code)
)]
mod finalize;
// These two carry the adapters' shared OAI protocol plumbing, so they are dead
// unless an adapter that speaks that protocol is enabled. The consumers are
// `openai`, `xai`, `azure` and `mistral` — *not* `openai-compat`, which is an
// internal umbrella that `openai`/`xai` imply and that enables no adapter on
// its own. Naming the umbrella here left the modules warning under
// `--features openai-compat`.
#[cfg_attr(
    not(any(
        feature = "openai",
        feature = "xai",
        feature = "azure",
        feature = "mistral",
    )),
    allow(dead_code)
)]
mod oai_transport;
#[cfg_attr(
    not(any(
        feature = "openai",
        feature = "xai",
        feature = "azure",
        feature = "mistral",
    )),
    allow(dead_code)
)]
mod openai_compat;
mod remote_presets;
pub mod sse;

pub use remote_presets::{
    RemoteModelConnectionError, RemotePresetKey, all_remote_presets, build_connection_from_preset,
    build_remote_connection, build_remote_connection_for_model,
    build_remote_connection_with_credential, is_provider_compiled, preset, remote_presets,
};

// ── Provider adapters (feature-gated) ─────────────────────────────────────
//
// Each adapter is gated behind its own feature flag. Enable the corresponding
// feature in Cargo.toml to compile and use a provider:
//
//   swink-agent-adapters = { features = ["anthropic", "openai"] }

#[cfg(feature = "anthropic")]
mod anthropic;
#[cfg(feature = "anthropic")]
pub use anthropic::AnthropicStreamFn;

// Gated on `openai`, not `openai-compat`: the only consumer of this module is
// the `openai`-gated re-export below. `xai` also enables `openai-compat`, but
// builds on `oai_transport` directly and never touches this module — so gating
// on `openai-compat` left the whole module dead under `xai`/`openai-compat`.
#[cfg(feature = "openai")]
#[allow(clippy::doc_markdown)]
mod openai;
#[cfg(feature = "openai")]
pub use openai::OpenAiStreamFn;

#[cfg(feature = "ollama")]
mod ollama;
#[cfg(feature = "ollama")]
pub use ollama::OllamaStreamFn;

#[cfg(feature = "gemini")]
mod google;
#[cfg(feature = "gemini")]
pub use google::GeminiStreamFn;

#[cfg(feature = "proxy")]
mod proxy;
#[cfg(feature = "proxy")]
pub use proxy::ProxyStreamFn;

#[cfg(feature = "azure")]
mod azure;
#[cfg(feature = "azure")]
pub use azure::{AzureAuth, AzureCloud, AzureStreamFn};

#[cfg(feature = "bedrock")]
mod bedrock;
#[cfg(feature = "bedrock")]
pub use bedrock::BedrockStreamFn;

#[cfg(feature = "mistral")]
mod mistral;
#[cfg(feature = "mistral")]
pub use mistral::MistralStreamFn;

#[cfg(feature = "xai")]
mod xai;
#[cfg(feature = "xai")]
pub use xai::XAiStreamFn;
